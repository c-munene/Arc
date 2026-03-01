use arc_common::{ArcError, Result};
use bytes::Bytes;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RouteMatcherSpec {
    Method { methods: Vec<String> },
    Host { hosts: Vec<String> },
    Sni { hosts: Vec<String> },
    HeaderPresent { name: String },
    HeaderEquals { name: String, value: String },
    QueryEquals { key: String, value: String },
    Tls { enabled: bool },
    H2 { enabled: bool },
}

#[derive(Debug, Deserialize, Clone)]
pub struct RouteHeaderSpec {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RouteActionSpec {
    Forward,
    Respond {
        status: u16,
        #[serde(default)]
        body: String,
        #[serde(default)]
        headers: Vec<RouteHeaderSpec>,
    },
}

impl Default for RouteActionSpec {
    fn default() -> Self {
        Self::Forward
    }
}

#[derive(Clone, Debug)]
pub enum RouteMatcher {
    Method { methods: Vec<Bytes> },
    Host { hosts: Vec<Bytes> },
    Sni { hosts: Vec<Bytes> },
    HeaderPresent { name: Bytes },
    HeaderEquals { name: Bytes, value: Bytes },
    QueryEquals { key: Bytes, value: Bytes },
    Tls { enabled: bool },
    H2 { enabled: bool },
}

#[derive(Clone, Debug)]
pub enum RouteAction {
    Forward,
    Respond {
        status: u16,
        http1_bytes: Bytes,
        h2_body: Bytes,
        h2_headers: Arc<[(Bytes, Bytes)]>,
    },
}

pub fn compile_matchers(specs: &[RouteMatcherSpec]) -> Result<Vec<RouteMatcher>> {
    let mut out = Vec::with_capacity(specs.len());
    for m in specs {
        let compiled = match m {
            RouteMatcherSpec::Method { methods } => {
                if methods.is_empty() {
                    return Err(ArcError::config(
                        "route matcher method.methods must not be empty".to_string(),
                    ));
                }
                let mut compiled = Vec::with_capacity(methods.len());
                for v in methods {
                    let s = v.trim();
                    if s.is_empty() {
                        return Err(ArcError::config(
                            "route matcher method.methods contains empty value".to_string(),
                        ));
                    }
                    compiled.push(Bytes::copy_from_slice(s.as_bytes()));
                }
                RouteMatcher::Method { methods: compiled }
            }
            RouteMatcherSpec::Host { hosts } => {
                if hosts.is_empty() {
                    return Err(ArcError::config(
                        "route matcher host.hosts must not be empty".to_string(),
                    ));
                }
                let mut compiled = Vec::with_capacity(hosts.len());
                for v in hosts {
                    let s = v.trim();
                    if s.is_empty() {
                        return Err(ArcError::config(
                            "route matcher host.hosts contains empty value".to_string(),
                        ));
                    }
                    compiled.push(Bytes::copy_from_slice(s.as_bytes()));
                }
                RouteMatcher::Host { hosts: compiled }
            }
            RouteMatcherSpec::Sni { hosts } => {
                if hosts.is_empty() {
                    return Err(ArcError::config(
                        "route matcher sni.hosts must not be empty".to_string(),
                    ));
                }
                let mut compiled = Vec::with_capacity(hosts.len());
                for v in hosts {
                    let s = v.trim();
                    if s.is_empty() {
                        return Err(ArcError::config(
                            "route matcher sni.hosts contains empty value".to_string(),
                        ));
                    }
                    compiled.push(Bytes::copy_from_slice(s.as_bytes()));
                }
                RouteMatcher::Sni { hosts: compiled }
            }
            RouteMatcherSpec::HeaderPresent { name } => {
                let n = name.trim();
                if n.is_empty() {
                    return Err(ArcError::config(
                        "route matcher header_present.name must not be empty".to_string(),
                    ));
                }
                RouteMatcher::HeaderPresent {
                    name: Bytes::copy_from_slice(n.as_bytes()),
                }
            }
            RouteMatcherSpec::HeaderEquals { name, value } => {
                let n = name.trim();
                if n.is_empty() {
                    return Err(ArcError::config(
                        "route matcher header_equals.name must not be empty".to_string(),
                    ));
                }
                RouteMatcher::HeaderEquals {
                    name: Bytes::copy_from_slice(n.as_bytes()),
                    value: Bytes::copy_from_slice(value.as_bytes()),
                }
            }
            RouteMatcherSpec::QueryEquals { key, value } => {
                let k = key.trim();
                if k.is_empty() {
                    return Err(ArcError::config(
                        "route matcher query_equals.key must not be empty".to_string(),
                    ));
                }
                RouteMatcher::QueryEquals {
                    key: Bytes::copy_from_slice(k.as_bytes()),
                    value: Bytes::copy_from_slice(value.as_bytes()),
                }
            }
            RouteMatcherSpec::Tls { enabled } => RouteMatcher::Tls { enabled: *enabled },
            RouteMatcherSpec::H2 { enabled } => RouteMatcher::H2 { enabled: *enabled },
        };
        out.push(compiled);
    }
    Ok(out)
}

pub fn compile_action(spec: &RouteActionSpec) -> Result<RouteAction> {
    match spec {
        RouteActionSpec::Forward => Ok(RouteAction::Forward),
        RouteActionSpec::Respond {
            status,
            body,
            headers,
        } => {
            if *status < 100 || *status > 599 {
                return Err(ArcError::config(format!(
                    "route action respond.status must be 100..=599, got {}",
                    status
                )));
            }
            let mut h = Vec::with_capacity(headers.len());
            for kv in headers {
                let name = kv.name.trim();
                if name.is_empty() {
                    return Err(ArcError::config(
                        "route action respond.headers contains empty name".to_string(),
                    ));
                }
                h.push((
                    Bytes::copy_from_slice(name.as_bytes()),
                    Bytes::copy_from_slice(kv.value.as_bytes()),
                ));
            }
            let http1_bytes = build_http1_response_bytes(*status, body.as_bytes(), &h);
            let h2_body = Bytes::copy_from_slice(body.as_bytes());
            let h2_headers: Arc<[(Bytes, Bytes)]> = h.into();
            Ok(RouteAction::Respond {
                status: *status,
                http1_bytes,
                h2_body,
                h2_headers,
            })
        }
    }
}

pub fn build_http1_response_bytes(
    status: u16,
    body: &[u8],
    extra_headers: &[(Bytes, Bytes)],
) -> Bytes {
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
