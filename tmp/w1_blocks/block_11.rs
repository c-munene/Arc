            let route_id = match self.active_cfg.router.at(path) {
                Some(r) => r,
                None => {
                    self.h2_release_body_parts(body_parts);
                    let _ = down.send_response_headers(sid, 404, vec![], true);
                    continue;
                }
            };
            let (upstream_id, limiter, rate_limit_policy, plugin_ids) = {
                let route = &self.active_cfg.routes[route_id as usize];
                (
                    route.upstream_id,
                    route.limiter.clone(),
                    route.rate_limit_policy,
                    route.plugin_ids.to_vec(),
                )
            };

            if self.upstream_circuit_open(upstream_id) {
                ...
            }

            if !Self::allow_route_rate_limit(... limiter.as_ref() ...) {
                ...
            }

            if let Some(plugins) = self.plugins.as_mut() {
                ...
            }

            let body = match self.h2_collect_body_bytes(body_parts, 8 * 1024 * 1024) {
                ...
            };

            let upstream_addr = self.active_cfg.upstreams[upstream_id].addr;
            let req = Self::h2_build_h1_request(&head, &body, upstream_addr);
            if self.upstream_tls.get(upstream_id).and_then(|v| v.as_ref()).is_some()
            {
                match self.h2_roundtrip_h1(upstream_id, &req) {
                    Ok((status, headers, body)) => {
                        self.mark_upstream_success(upstream_id);
                        self.h2_send_full_response(h2_down_key, sid, status, headers, body);
                    }
                    ...
                }
                let _ = self.h2_try_flush_downstream(h2_down_key);
                continue;
            }
