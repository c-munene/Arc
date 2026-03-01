            // multi-dim route select (candidates + matchers + priority + ambiguity detect)
            let route_id = match self.select_route_h2(
                method,
                path,
                head.authority.as_ref().map(|v| v.as_ref()),
                &head.headers,
                None, // sni: optional, but H2 is downstream TLS so you can pass sni if you want (see note below)
                true, // is_tls
            ) {
                Ok(r) => r,
                Err(RouteSelectError::NotFound) => {
                    self.h2_release_body_parts(body_parts);
                    let _ = down.send_response_headers(sid, 404, vec![], true);
                    continue;
                }
                Err(RouteSelectError::Ambiguous) => {
                    self.h2_release_body_parts(body_parts);
                    let _ = down.send_response_headers(sid, 503, vec![], true);
                    continue;
                }
            };

            let Some(route) = self.active_cfg.routes.get(route_id as usize) else {
                self.h2_release_body_parts(body_parts);
                let _ = down.send_response_headers(sid, 404, vec![], true);
                continue;
            };

            // circuit only for Forward
            let upstream_id = route.upstream_id;
            if matches!(route.action, RouteAction::Forward) && self.upstream_circuit_open(upstream_id) {
                self.h2_release_body_parts(body_parts);
                let _ = down.send_response_headers(sid, 503, vec![], true);
                continue;
            }

            if !Self::allow_route_rate_limit(
                self.global_limiter.as_mut(),
                route_id,
                route.rate_limit_policy,
                route.limiter.as_ref(),
                now_ns,
            ) {
                self.h2_release_body_parts(body_parts);
                let _ = down.send_response_headers(sid, 429, vec![], true);
                continue;
            }

            if let Some(plugins) = self.plugins.as_mut() {
                let mut denied: Option<u16> = None;
                for pid in route.plugin_ids.iter().copied() {
                    let verdict =
                        plugins.exec_on_request(pid, arc_plugins::RequestView { method, path });
                    if !verdict.allowed {
                        denied = Some(verdict.deny_status.max(400));
                        break;
                    }
                }
                if let Some(code) = denied {
                    self.h2_release_body_parts(body_parts);
                    let _ = down.send_response_headers(sid, code, vec![], true);
                    continue;
                }
            }

            // action: Respond (no upstream)
            if let RouteAction::Respond { status, h2_body, .. } = &route.action {
                self.h2_release_body_parts(body_parts);
                if h2_body.is_empty() {
                    let _ = down.send_response_headers(sid, *status, vec![], true);
                } else {
                    self.h2_send_full_response_on_down(down, sid, *status, vec![], h2_body.as_ref());
                }
                continue;
            }

            // forward: collect body and build H1 request
            let body = match self.h2_collect_body_bytes(body_parts, 8 * 1024 * 1024) {
                Ok(v) => v,
                Err(_) => {
                    let _ = down.send_response_headers(sid, 413, vec![], true);
                    continue;
                }
            };

            let upstream_addr = self.active_cfg.upstreams[upstream_id].addr;
            let req = Self::h2_build_h1_request(&head, &body, upstream_addr);

            // upstream TLS sync path: send directly on `down` (fix bug)
            if self
                .upstream_tls
                .get(upstream_id)
                .and_then(|v| v.as_ref())
                .is_some()
            {
                match self.h2_roundtrip_h1(upstream_id, &req) {
                    Ok((status, headers, body)) => {
                        self.mark_upstream_success(upstream_id);
                        self.h2_send_full_response_on_down(down, sid, status, headers, &body);
                    }
                    Err(H2H1RoundtripError::Timeout) => {
                        self.mark_upstream_failure(upstream_id);
                        let _ = down.send_response_headers(sid, 504, vec![], true);
                    }
                    Err(_) => {
                        self.mark_upstream_failure(upstream_id);
                        let _ = down.send_response_headers(sid, 502, vec![], true);
                    }
                }
                continue;
            }
