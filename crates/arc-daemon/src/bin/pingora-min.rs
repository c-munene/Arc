//! Minimal Pingora HTTP proxy used for baseline benchmarking.

use pingora::prelude::*;
use pingora::proxy::{http_proxy_service, ProxyHttp};
use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Clone)]
struct MinProxy {
    upstream: std::net::SocketAddr,
}

impl ProxyHttp for MinProxy {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        Ok(Box::new(HttpPeer::new(self.upstream, false, "".to_string())))
    }
}

fn main() {
    let bind = std::env::var("PINGORA_MIN_BIND").unwrap_or_else(|_| "127.0.0.1:8082".to_string());
    let upstream_raw =
        std::env::var("PINGORA_MIN_UPSTREAM").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let upstream = upstream_raw
        .parse::<std::net::SocketAddr>()
        .expect("invalid PINGORA_MIN_UPSTREAM");
    let workers = std::env::var("PINGORA_MIN_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        });

    let mut server = Server::new(None).expect("failed to create pingora server");
    {
        let conf = Arc::get_mut(&mut server.configuration)
            .expect("server config should be uniquely owned before bootstrap");
        conf.threads = workers.max(1);
        conf.upstream_keepalive_pool_size = 1024;
    }
    server.bootstrap();

    let mut svc = http_proxy_service(
        &server.configuration,
        MinProxy {
            upstream,
        },
    );
    svc.add_tcp(&bind);
    server.add_service(svc);
    server.run_forever();
}
