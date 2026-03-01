impl Worker {
    #[inline]
    fn select_route_http1(
        &self,
        method: &[u8],
        full_path: &[u8],
        head_block: &[u8],
        sni: Option<&[u8]>,
        is_tls: bool,
    ) -> std::result::Result<u32, RouteSelectError> {
        let path_no_q = strip_query(full_path);

        let host = http1_header_value(head_block, b"host")
            .map(trim_ascii_ws)
            .map(host_without_port)
            .map(trim_trailing_dot);

        let mut best_pri: i32 = i32::MIN;
        let mut best_spec: u32 = 0;
        let mut best_id: u32 = 0;
        let mut best_count: u32 = 0;

        self.active_cfg.router.for_each_candidate(path_no_q, |rid| {
            let Some(route) = self.active_cfg.routes.get(rid as usize) else {
                return;
            };

            if !route_matches_http1(route, method, full_path, head_block, host, sni, is_tls) {
                return;
            }

            let pri = route.priority;
            let spec = route.specificity();

            if pri > best_pri || (pri == best_pri && spec > best_spec) {
                best_pri = pri;
                best_spec = spec;
                best_id = rid;
                best_count = 1;
            } else if pri == best_pri && spec == best_spec {
                best_count = best_count.saturating_add(1);
            }
        });

        if best_count == 0 {
            return Err(RouteSelectError::NotFound);
        }
        if best_count > 1 {
            return Err(RouteSelectError::Ambiguous);
        }
        Ok(best_id)
    }

    #[inline]
    fn select_route_h2(
        &self,
        method: &[u8],
        full_path: &[u8],
        authority: Option<&[u8]>,
        headers: &[H2Header],
        sni: Option<&[u8]>,
        is_tls: bool,
    ) -> std::result::Result<u32, RouteSelectError> {
        let path_no_q = strip_query(full_path);

        let host = authority
            .or_else(|| h2_header_value(headers, b"host"))
            .map(trim_ascii_ws)
            .map(host_without_port)
            .map(trim_trailing_dot);

        let mut best_pri: i32 = i32::MIN;
        let mut best_spec: u32 = 0;
        let mut best_id: u32 = 0;
        let mut best_count: u32 = 0;

        self.active_cfg.router.for_each_candidate(path_no_q, |rid| {
            let Some(route) = self.active_cfg.routes.get(rid as usize) else {
                return;
            };

            if !route_matches_h2(route, method, full_path, headers, host, sni, is_tls) {
                return;
            }

            let pri = route.priority;
            let spec = route.specificity();

            if pri > best_pri || (pri == best_pri && spec > best_spec) {
                best_pri = pri;
                best_spec = spec;
                best_id = rid;
                best_count = 1;
            } else if pri == best_pri && spec == best_spec {
                best_count = best_count.saturating_add(1);
            }
        });

        if best_count == 0 {
            return Err(RouteSelectError::NotFound);
        }
        if best_count > 1 {
            return Err(RouteSelectError::Ambiguous);
        }
        Ok(best_id)
    }

    /// H2 直接对 `&mut DownstreamH2` 发送完整响应（解决 conn.h2_down.take() 期间无法发送的问题）。
    fn h2_send_full_response_on_down(
        &mut self,
        down: &mut DownstreamH2,
        sid: u32,
        status: u16,
        headers: Vec<H2Header>,
        body: &[u8],
    ) {
        if body.is_empty() {
            let _ = down.send_response_headers(sid, status, headers, true);
            return;
        }

        let Some(chain) = self.h2_body_to_chain(body) else {
            let _ = down.send_response_headers(sid, 503, vec![], true);
            return;
        };

        if down.send_response_headers(sid, status, headers, false).is_err() {
            let mut ops = WorkerH2BufOps { bufs: &mut self.bufs };
            let mut c = chain;
            c.release(&mut ops);
            return;
        }

        let mut ops = WorkerH2BufOps { bufs: &mut self.bufs };
        let _ = down.send_response_data(sid, true, chain, None, &mut ops);
    }
}
