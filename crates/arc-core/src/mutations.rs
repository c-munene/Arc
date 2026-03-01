use crate::config::{HeaderMutation, RewriteRule};
use http::{HeaderMap, HeaderName, HeaderValue};
use regex::Regex;

/// Apply a regex rewrite rule to a path.
pub fn rewrite_path(path: &str, rule: &RewriteRule) -> anyhow::Result<Option<String>> {
    let re = Regex::new(&rule.pattern)?;
    if !re.is_match(path) {
        return Ok(None);
    }
    Ok(Some(re.replace(path, rule.replace.as_str()).to_string()))
}

/// Apply header mutations to an `http::HeaderMap`.
///
/// For Pingora integration, convert between Pingora's header types and `http` types.
pub fn apply_header_mutations(
    headers: &mut HeaderMap,
    muts: &[HeaderMutation],
) -> anyhow::Result<()> {
    for m in muts {
        match m {
            HeaderMutation::Add { name, value } => {
                let n = HeaderName::from_bytes(name.as_bytes())?;
                let v = HeaderValue::from_str(value)?;
                headers.append(n, v);
            }
            HeaderMutation::Set { name, value } => {
                let n = HeaderName::from_bytes(name.as_bytes())?;
                let v = HeaderValue::from_str(value)?;
                headers.insert(n, v);
            }
            HeaderMutation::Remove { name } => {
                let n = HeaderName::from_bytes(name.as_bytes())?;
                headers.remove(n);
            }
        }
    }
    Ok(())
}
