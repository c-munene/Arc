// arc_config/src/route_rules.rs
use bytes::Bytes;

use crate::RateLimitPolicy;

#[derive(Clone, Debug)]
pub enum RouteMatcher {
    Method { methods: Vec<Bytes> },

    /// Match against HTTP/1 Host header; if absent, can fallback to SNI (worker-side).
    Host { hosts: Vec<Bytes> },

    /// Match against downstream TLS SNI (lower/trim handled by worker).
    Sni { hosts: Vec<Bytes> },

    HeaderPresent { name: Bytes },
    HeaderEquals { name: Bytes, value: Bytes },

    QueryEquals { key: Bytes, value: Bytes },

    /// Require TLS or plain.
    Tls { enabled: bool },

    /// Require H2 or H1 (worker decides based on connection mode).
    H2 { enabled: bool },
}

#[derive(Clone, Debug)]
pub enum RouteAction {
    /// Normal proxy to upstream (existing behavior).
    Forward,

    /// Direct response (dataplane 不回源)：
    /// - HTTP/1: 直接把 http1_bytes copy 到 fixed buffer 并 close
    /// - H2: status + (optional) body
    Respond {
        status: u16,
        http1_bytes: Bytes,
        h2_body: Bytes,
    },
}

#[derive(Clone, Debug)]
pub struct Route {
    /// Path prefix registered into arc_router::Router (bytes, e.g. b"/foo").
    pub path: Bytes,

    /// Higher wins.
    pub priority: i32,

    /// AND semantics.
    pub matchers: Vec<RouteMatcher>,

    pub action: RouteAction,

    // ---- existing fields used by worker ----
    pub upstream_id: usize,
    pub limiter: Option<std::sync::Arc<arc_rate_limit::Limiter>>,
    pub rate_limit_policy: Option<RateLimitPolicy>,
    pub plugin_ids: Vec<u32>,
}

impl Route {
    #[inline]
    pub fn specificity(&self) -> u32 {
        // path longer => more specific; matchers more => more specific
        (self.path.len() as u32) * 1024 + (self.matchers.len() as u32)
    }
}

/// Config-load helper: build a static HTTP/1 response (Connection: close).
/// 这是在“加载配置时”运行的，允许用 to_string 之类（不影响 dataplane 0-alloc）。
pub fn build_http1_response_bytes(status: u16, body: &[u8], extra_headers: &[(Bytes, Bytes)]) -> Bytes {
    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            413 => "Payload Too Large",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "OK",
        }
    }

    let mut out = Vec::with_capacity(128 + body.len() + extra_headers.len() * 32);

    out.extend_from_slice(b"HTTP/1.1 ");
    out.extend_from_slice(status.to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(reason(status).as_bytes());
    out.extend_from_slice(b"\r\n");

    out.extend_from_slice(b"Connection: close\r\n");
    out.extend_from_slice(b"Content-Length: ");
    out.extend_from_slice(body.len().to_string().as_bytes());
    out.extend_from_slice(b"\r\n");

    for (k, v) in extra_headers {
        out.extend_from_slice(k.as_ref());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_ref());
        out.extend_from_slice(b"\r\n");
    }

    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);

    Bytes::from(out)
}
