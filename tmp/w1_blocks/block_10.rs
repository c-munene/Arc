                // ---- capture SNI/TLS for multi-dim routing (copy to stack; no borrow across calls) ----
                let is_tls = conn.tls.is_some();
                let mut sni_tmp = [0u8; 256];
                let mut sni_len = 0usize;
                if let Some(h) = conn.sni_host.as_ref() {
                    let n = (conn.sni_len as usize).min(256);
                    sni_tmp[..n].copy_from_slice(&h[..n]);
                    sni_len = n;
                }
                let sni = if sni_len > 0 { Some(&sni_tmp[..sni_len]) } else { None };

                // drop conn borrow before calling routing/limits/plugins
                let _ = conn;

                // route selection: candidates + matchers + priority + ambiguity detect
                let route_id = match self.select_route_http1(
                    head.method,
                    head.path,
                    &buf_slice[..head.header_end],
                    sni,
                    is_tls,
                ) {
                    Ok(r) => r,
                    Err(RouteSelectError::NotFound) => {
                        self.queue_error_response(key, RESP_404)?;
                        return Ok(());
                    }
                    Err(RouteSelectError::Ambiguous) => {
                        self.queue_error_response(key, RESP_503)?;
                        return Ok(());
                    }
                };

                let Some(route) = self.active_cfg.routes.get(route_id as usize) else {
                    self.queue_error_response(key, RESP_404)?;
                    return Ok(());
                };

                // upstream circuit check only makes sense for Forward
                let upstream_id = route.upstream_id;
                if matches!(route.action, RouteAction::Forward) && self.upstream_circuit_open(upstream_id) {
                    self.queue_error_response(key, RESP_503)?;
                    return Ok(());
                }

                // rate limit (global if enabled, otherwise fallback to local limiter)
                if !Self::allow_route_rate_limit(
                    self.global_limiter.as_mut(),
                    route_id,
                    route.rate_limit_policy,
                    route.limiter.as_ref(),
                    now,
                ) {
                    self.queue_error_response(key, RESP_429)?;
                    return Ok(());
                }

                // plugin chain (no per-request Vec clone)
                if let Some(plugins) = self.plugins.as_mut() {
                    for pid in route.plugin_ids.iter().copied() {
                        let verdict = plugins.exec_on_request(
                            pid,
                            arc_plugins::RequestView {
                                method: head.method,
                                path: head.path,
                            },
                        );
                        if !verdict.allowed {
                            let resp = match verdict.deny_status {
                                503 => RESP_503,
                                429 => RESP_429,
                                404 => RESP_404,
                                400 => RESP_400,
                                502 => RESP_502,
                                504 => RESP_504,
                                _ => RESP_503,
                            };
                            self.queue_error_response(key, resp)?;
                            return Ok(());
                        }
                    }
                }

                // route action: Respond (direct response, no upstream)
                if let RouteAction::Respond { http1_bytes, .. } = &route.action {
                    self.queue_error_response(key, http1_bytes.as_ref())?;
                    return Ok(());
                }

                // ---- re-borrow conn to continue normal forward path ----
                let Some(conn) = self.conns.get_mut(key) else {
                    return Ok(());
                };

                conn.route_id = route_id;
                conn.upstream_id = upstream_id;
                conn.req_keepalive = head.keepalive;
                conn.up_write_retries = 0;
