#[inline]
fn strip_query(path: &[u8]) -> &[u8] {
    match path.iter().position(|b| *b == b'?') {
        Some(pos) => &path[..pos],
        None => path,
    }
}

#[inline]
fn trim_ascii_ws(mut s: &[u8]) -> &[u8] {
    while let Some(&b) = s.first() {
        if b.is_ascii_whitespace() {
            s = &s[1..];
        } else {
            break;
        }
    }
    while let Some(&b) = s.last() {
        if b.is_ascii_whitespace() {
            s = &s[..s.len().saturating_sub(1)];
        } else {
            break;
        }
    }
    s
}

#[inline]
fn trim_trailing_dot(mut s: &[u8]) -> &[u8] {
    while s.last() == Some(&b'.') {
        s = &s[..s.len().saturating_sub(1)];
    }
    s
}

#[inline]
fn host_without_port(host: &[u8]) -> &[u8] {
    let host = trim_ascii_ws(host);
    if host.is_empty() {
        return host;
    }

    // [::1]:443
    if host.first() == Some(&b'[') {
        if let Some(end) = host.iter().position(|b| *b == b']') {
            if end > 1 {
                return &host[1..end];
            }
        }
        return host;
    }

    // example.com:8080
    match host.iter().position(|b| *b == b':') {
        Some(pos) => &host[..pos],
        None => host,
    }
}

#[inline]
fn host_matches_any(host: &[u8], patterns: &[Bytes]) -> bool {
    if host.is_empty() {
        return false;
    }
    let host = trim_trailing_dot(host);

    for pat in patterns {
        let p = trim_trailing_dot(pat.as_ref());
        if p == b"*" {
            return true;
        }
        if p.len() >= 2 && p[0] == b'*' && p[1] == b'.' {
            let suf = &p[1..]; // ".example.com"
            if host.len() > suf.len()
                && host[host.len() - suf.len()..].eq_ignore_ascii_case(suf)
            {
                return true;
            }
        } else if host.eq_ignore_ascii_case(p) {
            return true;
        }
    }
    false
}

fn http1_header_value<'a>(head_block: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut pos = 0usize;
    let mut first_line = true;

    while pos < head_block.len() {
        let mut end = pos;
        while end < head_block.len() && head_block[end] != b'\n' {
            end += 1;
        }
        let mut line = &head_block[pos..end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len().saturating_sub(1)];
        }

        if first_line {
            first_line = false;
        } else if line.is_empty() {
            break;
        } else if let Some(colon) = line.iter().position(|b| *b == b':') {
            let n = &line[..colon];
            if n.eq_ignore_ascii_case(name) {
                let v = trim_ascii_ws(&line[colon + 1..]);
                return Some(v);
            }
        }

        pos = if end < head_block.len() { end + 1 } else { end };
    }

    None
}

fn query_param_value<'a>(path: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let q = path.iter().position(|b| *b == b'?')?;
    let mut i = q + 1;

    while i <= path.len() {
        let mut j = i;
        while j < path.len() && path[j] != b'&' {
            j += 1;
        }

        let pair = &path[i..j];
        if let Some(eq) = pair.iter().position(|b| *b == b'=') {
            if &pair[..eq] == key {
                return Some(&pair[eq + 1..]);
            }
        } else if pair == key {
            return Some(&[]);
        }

        if j >= path.len() {
            break;
        }
        i = j + 1;
    }
    None
}

fn h2_header_value<'a>(headers: &'a [H2Header], name: &[u8]) -> Option<&'a [u8]> {
    for h in headers {
        if h.name.as_ref().eq_ignore_ascii_case(name) {
            return Some(h.value.as_ref());
        }
    }
    None
}

// ---- matcher evaluators ----

fn route_matches_http1(
    route: &arc_config::Route,
    method: &[u8],
    full_path: &[u8],
    head_block: &[u8],
    host: Option<&[u8]>,
    sni: Option<&[u8]>,
    is_tls: bool,
) -> bool {
    for m in route.matchers.iter() {
        match m {
            RouteMatcher::Method { methods } => {
                if !methods.iter().any(|x| x.as_ref() == method) {
                    return false;
                }
            }
            RouteMatcher::Host { hosts } => {
                let h = host.or(sni).unwrap_or(&[]);
                if !host_matches_any(h, hosts) {
                    return false;
                }
            }
            RouteMatcher::Sni { hosts } => {
                let s = sni.unwrap_or(&[]);
                if !host_matches_any(s, hosts) {
                    return false;
                }
            }
            RouteMatcher::HeaderPresent { name } => {
                if http1_header_value(head_block, name.as_ref()).is_none() {
                    return false;
                }
            }
            RouteMatcher::HeaderEquals { name, value } => {
                let Some(v) = http1_header_value(head_block, name.as_ref()) else {
                    return false;
                };
                if trim_ascii_ws(v) != value.as_ref() {
                    return false;
                }
            }
            RouteMatcher::QueryEquals { key, value } => {
                let Some(v) = query_param_value(full_path, key.as_ref()) else {
                    return false;
                };
                if v != value.as_ref() {
                    return false;
                }
            }
            RouteMatcher::Tls { enabled } => {
                if *enabled != is_tls {
                    return false;
                }
            }
            RouteMatcher::H2 { enabled } => {
                // http1 path
                if *enabled {
                    return false;
                }
            }
        }
    }
    true
}

fn route_matches_h2(
    route: &arc_config::Route,
    method: &[u8],
    full_path: &[u8],
    headers: &[H2Header],
    host: Option<&[u8]>,
    sni: Option<&[u8]>,
    is_tls: bool,
) -> bool {
    for m in route.matchers.iter() {
        match m {
            RouteMatcher::Method { methods } => {
                if !methods.iter().any(|x| x.as_ref() == method) {
                    return false;
                }
            }
            RouteMatcher::Host { hosts } => {
                let h = host.or(sni).unwrap_or(&[]);
                if !host_matches_any(h, hosts) {
                    return false;
                }
            }
            RouteMatcher::Sni { hosts } => {
                let s = sni.unwrap_or(&[]);
                if !host_matches_any(s, hosts) {
                    return false;
                }
            }
            RouteMatcher::HeaderPresent { name } => {
                if h2_header_value(headers, name.as_ref()).is_none() {
                    return false;
                }
            }
            RouteMatcher::HeaderEquals { name, value } => {
                let Some(v) = h2_header_value(headers, name.as_ref()) else {
                    return false;
                };
                if trim_ascii_ws(v) != value.as_ref() {
                    return false;
                }
            }
            RouteMatcher::QueryEquals { key, value } => {
                let Some(v) = query_param_value(full_path, key.as_ref()) else {
                    return false;
                };
                if v != value.as_ref() {
                    return false;
                }
            }
            RouteMatcher::Tls { enabled } => {
                if *enabled != is_tls {
                    return false;
                }
            }
            RouteMatcher::H2 { enabled } => {
                // h2 path
                if !*enabled {
                    return false;
                }
            }
        }
    }
    true
}
