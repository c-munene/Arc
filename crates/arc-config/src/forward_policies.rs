use arc_common::{ArcError, Result};
use bytes::Bytes;
use serde::Deserialize;
use std::collections::HashMap;
use std::{sync::Arc, time::Duration};

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TrafficSplitConfig {
    pub choices: Vec<TrafficChoice>,
    #[serde(default)]
    pub key: SplitKey,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceConfig {
    Random,
    Weighted,
    Hash,
    RoundRobin,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TrafficChoice {
    pub upstream: String,
    pub weight: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SplitKey {
    Random,
    Path,
    Host,
    Header { name: String },
    Cookie { name: String },
}

impl Default for SplitKey {
    fn default() -> Self {
        Self::Path
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RewriteRule {
    pub pattern: String,
    pub replace: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HeaderMutation {
    Add { name: String, value: String },
    Set { name: String, value: String },
    Remove { name: String },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(with = "humantime_serde", default = "default_backoff")]
    pub backoff: Duration,
    #[serde(default = "default_idempotent_only")]
    pub idempotent_only: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_retries(),
            backoff: default_backoff(),
            idempotent_only: default_idempotent_only(),
        }
    }
}

fn default_retries() -> u32 {
    1
}

fn default_backoff() -> Duration {
    Duration::from_millis(50)
}

fn default_idempotent_only() -> bool {
    true
}

#[derive(Clone, Debug)]
pub enum RouteUpstreams {
    None,
    Single { upstream_id: usize },
    Split(Arc<CompiledSplit>),
}

#[derive(Clone, Debug)]
pub struct CompiledSplit {
    pub key: CompiledSplitKey,
    pub load_balance: CompiledLoadBalance,
    pub total_weight: u32,
    pub choices: Arc<[SplitChoice]>,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub enum CompiledLoadBalance {
    HashWeighted,
    RoundRobin,
}

#[derive(Clone, Debug)]
pub struct SplitChoice {
    pub upstream_id: usize,
    pub cumulative: u32,
}

#[derive(Clone, Debug)]
pub enum CompiledSplitKey {
    Random,
    Path,
    Host,
    Header(Bytes),
    Cookie(Bytes),
}

#[derive(Clone, Debug)]
pub struct ForwardPolicy {
    pub rewrite: Option<CompiledRewrite>,
    pub header_muts: Arc<[CompiledHeaderMutation]>,
    pub retry: CompiledRetryPolicy,
}

impl Default for ForwardPolicy {
    fn default() -> Self {
        Self {
            rewrite: None,
            header_muts: Arc::from([]),
            retry: CompiledRetryPolicy::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledRetryPolicy {
    pub max_retries: u32,
    pub backoff_ns: u64,
    pub idempotent_only: bool,
}

impl Default for CompiledRetryPolicy {
    fn default() -> Self {
        let p = RetryPolicy::default();
        Self::from_model(&p)
    }
}

impl CompiledRetryPolicy {
    pub fn from_model(p: &RetryPolicy) -> Self {
        let max_retries = p.max_retries.min(8);
        let backoff_ns = p.backoff.as_nanos().min(Duration::from_secs(5).as_nanos()) as u64;
        Self {
            max_retries,
            backoff_ns,
            idempotent_only: p.idempotent_only,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CompiledHeaderMutation {
    Add {
        name: Bytes,
        name_lower: Bytes,
        value: Bytes,
    },
    Set {
        name: Bytes,
        name_lower: Bytes,
        value: Bytes,
    },
    Remove {
        name_lower: Bytes,
    },
}

#[derive(Clone, Debug)]
pub enum CompiledRewrite {
    Prefix {
        from: Bytes,
        to: Bytes,
    },
    Regex {
        re: Arc<regex::bytes::Regex>,
        replace: Bytes,
    },
}

pub fn compile_upstreams(
    is_forward: bool,
    upstream: Option<&str>,
    split: Option<&TrafficSplitConfig>,
    load_balance: Option<LoadBalanceConfig>,
    upstream_name_to_id: &HashMap<&str, usize>,
) -> Result<RouteUpstreams> {
    if !is_forward {
        return Ok(RouteUpstreams::None);
    }

    if let Some(s) = split {
        if s.choices.is_empty() {
            return Err(ArcError::config(
                "split.choices must be non-empty".to_string(),
            ));
        }

        let key = compile_split_key(&s.key)?;
        let lb = match load_balance.unwrap_or(LoadBalanceConfig::Weighted) {
            LoadBalanceConfig::RoundRobin => CompiledLoadBalance::RoundRobin,
            LoadBalanceConfig::Random | LoadBalanceConfig::Weighted | LoadBalanceConfig::Hash => {
                CompiledLoadBalance::HashWeighted
            }
        };
        let mut out: Vec<SplitChoice> = Vec::with_capacity(s.choices.len());
        let mut total: u32 = 0;

        for c in &s.choices {
            if c.weight == 0 {
                return Err(ArcError::config(format!(
                    "split choice weight must be > 0 (upstream={})",
                    c.upstream
                )));
            }
            let up_id = upstream_name_to_id
                .get(c.upstream.as_str())
                .copied()
                .ok_or_else(|| {
                    ArcError::config(format!("unknown upstream in split: {}", c.upstream))
                })?;
            total = total
                .checked_add(c.weight)
                .ok_or_else(|| ArcError::config("split total weight overflow".to_string()))?;
            out.push(SplitChoice {
                upstream_id: up_id,
                cumulative: total,
            });
        }

        return Ok(RouteUpstreams::Split(Arc::new(CompiledSplit {
            key,
            load_balance: lb,
            total_weight: total,
            choices: out.into(),
        })));
    }

    if let Some(name) = upstream {
        let up_id = upstream_name_to_id
            .get(name)
            .copied()
            .ok_or_else(|| ArcError::config(format!("unknown upstream in route: {}", name)))?;
        return Ok(RouteUpstreams::Single { upstream_id: up_id });
    }

    Err(ArcError::config(
        "forward route must specify either `upstream` or `split`".to_string(),
    ))
}

pub fn compile_forward_policy(
    rewrite: Option<&RewriteRule>,
    headers: &[HeaderMutation],
    retry: &RetryPolicy,
) -> Result<ForwardPolicy> {
    let compiled_rewrite = compile_rewrite(rewrite)?;
    let compiled_headers = compile_header_mutations(headers)?;
    let compiled_retry = CompiledRetryPolicy::from_model(retry);
    Ok(ForwardPolicy {
        rewrite: compiled_rewrite,
        header_muts: compiled_headers,
        retry: compiled_retry,
    })
}

fn compile_split_key(k: &SplitKey) -> Result<CompiledSplitKey> {
    Ok(match k {
        SplitKey::Random => CompiledSplitKey::Random,
        SplitKey::Path => CompiledSplitKey::Path,
        SplitKey::Host => CompiledSplitKey::Host,
        SplitKey::Header { name } => {
            let n = name.trim();
            if n.is_empty() {
                return Err(ArcError::config(
                    "split.key.header.name is empty".to_string(),
                ));
            }
            CompiledSplitKey::Header(Bytes::from(ascii_lower_bytes(n.as_bytes())))
        }
        SplitKey::Cookie { name } => {
            let n = name.trim();
            if n.is_empty() {
                return Err(ArcError::config(
                    "split.key.cookie.name is empty".to_string(),
                ));
            }
            CompiledSplitKey::Cookie(Bytes::copy_from_slice(n.as_bytes()))
        }
    })
}

fn compile_rewrite(rule: Option<&RewriteRule>) -> Result<Option<CompiledRewrite>> {
    let Some(r) = rule else {
        return Ok(None);
    };
    let pat = r.pattern.trim();
    if pat.is_empty() {
        return Err(ArcError::config("rewrite.pattern is empty".to_string()));
    }
    let rep = r.replace.as_str();

    if let Some(from) = try_parse_simple_prefix_regex(pat) {
        if rep.as_bytes().contains(&b'$') {
            return Err(ArcError::config(
                "prefix-like rewrite does not support capture replacement ($)".to_string(),
            ));
        }
        return Ok(Some(CompiledRewrite::Prefix {
            from: Bytes::copy_from_slice(from),
            to: Bytes::copy_from_slice(rep.as_bytes()),
        }));
    }

    let re = regex::bytes::Regex::new(pat)
        .map_err(|e| ArcError::config(format!("invalid rewrite regex: {e}")))?;
    Ok(Some(CompiledRewrite::Regex {
        re: Arc::new(re),
        replace: Bytes::copy_from_slice(rep.as_bytes()),
    }))
}

fn try_parse_simple_prefix_regex(pat: &str) -> Option<&[u8]> {
    if !pat.starts_with('^') {
        return None;
    }
    let lit = &pat[1..];
    if lit.is_empty() {
        return None;
    }
    if lit.bytes().any(|b| {
        matches!(
            b,
            b'.' | b'+'
                | b'*'
                | b'?'
                | b'['
                | b']'
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'|'
                | b'\\'
                | b'$'
        )
    }) {
        return None;
    }
    Some(lit.as_bytes())
}

fn compile_header_mutations(muts: &[HeaderMutation]) -> Result<Arc<[CompiledHeaderMutation]>> {
    if muts.is_empty() {
        return Ok(Arc::from([]));
    }

    const FORBIDDEN: [&[u8]; 8] = [
        b"connection",
        b"proxy-connection",
        b"keep-alive",
        b"transfer-encoding",
        b"upgrade",
        b"te",
        b"trailer",
        b"content-length",
    ];

    let mut out: Vec<CompiledHeaderMutation> = Vec::with_capacity(muts.len());
    for m in muts {
        match m {
            HeaderMutation::Add { name, value } => {
                let (n, n_lower, v) = compile_header_kv(name, value, &FORBIDDEN)?;
                out.push(CompiledHeaderMutation::Add {
                    name: n,
                    name_lower: n_lower,
                    value: v,
                });
            }
            HeaderMutation::Set { name, value } => {
                let (n, n_lower, v) = compile_header_kv(name, value, &FORBIDDEN)?;
                out.push(CompiledHeaderMutation::Set {
                    name: n,
                    name_lower: n_lower,
                    value: v,
                });
            }
            HeaderMutation::Remove { name } => {
                let n = name.trim();
                if n.is_empty() {
                    return Err(ArcError::config("header remove name is empty".to_string()));
                }
                let lower = ascii_lower_bytes(n.as_bytes());
                if FORBIDDEN.iter().any(|&f| eq_ascii_case(&lower, f)) {
                    return Err(ArcError::config(format!(
                        "refuse to mutate hop-by-hop/framing header: {}",
                        n
                    )));
                }
                out.push(CompiledHeaderMutation::Remove {
                    name_lower: Bytes::from(lower),
                });
            }
        }
    }
    Ok(out.into())
}

fn compile_header_kv(
    name: &str,
    value: &str,
    forbidden: &[&[u8]],
) -> Result<(Bytes, Bytes, Bytes)> {
    let n = name.trim();
    if n.is_empty() {
        return Err(ArcError::config(
            "header mutation name is empty".to_string(),
        ));
    }
    if !is_http_token(n.as_bytes()) {
        return Err(ArcError::config(format!(
            "invalid header name token: {}",
            n
        )));
    }

    let lower = ascii_lower_bytes(n.as_bytes());
    if forbidden.iter().any(|&f| eq_ascii_case(&lower, f)) {
        return Err(ArcError::config(format!(
            "refuse to mutate hop-by-hop/framing header: {}",
            n
        )));
    }

    let v = value.as_bytes();
    if v.iter().any(|&b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(ArcError::config(format!(
            "invalid header value (CR/LF/NUL) for header: {}",
            n
        )));
    }

    Ok((
        Bytes::copy_from_slice(n.as_bytes()),
        Bytes::from(lower),
        Bytes::copy_from_slice(v),
    ))
}

fn ascii_lower_bytes(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len());
    for &b in s {
        v.push(if b.is_ascii_uppercase() { b + 32 } else { b });
    }
    v
}

fn eq_ascii_case(a_lower: &[u8], b_lower: &[u8]) -> bool {
    a_lower == b_lower
}

fn is_http_token(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    s.iter().all(|&b| {
        matches!(
            b,
            b'0'..=b'9'
                | b'a'..=b'z'
                | b'A'..=b'Z'
                | b'!'
                | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
    })
}
