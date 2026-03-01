                // route
                let route_id = match self.active_cfg.router.at(head.path) {
                    Some(r) => r,
                    None => {
                        self.queue_error_response(key, RESP_404)?;
                        return Ok(());
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

                let _ = conn;
                if self.upstream_circuit_open(upstream_id) {
                    self.queue_error_response(key, RESP_503)?;
                    return Ok(());
                }

                // rate limit (global if enabled, otherwise fallback to local limiter)
                if !Self::allow_route_rate_limit(
                    self.global_limiter.as_mut(),
                    route_id,
                    rate_limit_policy,
                    limiter.as_ref(),
                    now,
                ) {
                    self.queue_error_response(key, RESP_429)?;
                    return Ok(());
                }

                // plugin chain
                if let Some(plugins) = self.plugins.as_mut() {
                    for pid in plugin_ids.iter().copied() {
                        ...
                    }
                }

                let Some(conn) = self.conns.get_mut(key) else {
                    return Ok(());
                };

                // select upstream
                conn.route_id = route_id;
                conn.upstream_id = upstream_id;
