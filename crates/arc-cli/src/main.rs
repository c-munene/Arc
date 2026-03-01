use clap::{Args, Parser, Subcommand};
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Parser, Debug)]
#[command(name = "arc")]
#[command(about = "Arc CLI (logs tail/query, local node only)", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(subcommand)]
    Logs(LogsCmd),
}

#[derive(Subcommand, Debug)]
enum LogsCmd {
    #[command(
        about = "Tail access logs (local node only)",
        long_about = "Arc logs commands read local files only. For cluster-wide logs, use external collectors (Vector/Fluent Bit) and a central backend (Loki/Elasticsearch).",
        after_help = "Filtering runs in the CLI process (client-side): it reads and parses lines locally, then applies filters. Under high sampling/high QPS, prefer --last to narrow scan range."
    )]
    Tail(TailArgs),
    #[command(
        about = "Query access logs (local node only)",
        long_about = "Arc logs commands read local files only. For cluster-wide logs, use external collectors (Vector/Fluent Bit) and a central backend (Loki/Elasticsearch).",
        after_help = "Filtering runs in the CLI process (client-side): it reads and parses lines locally, then applies filters. Under high sampling/high QPS, prefer --last to narrow scan range."
    )]
    Query(QueryArgs),
}

#[derive(Args, Debug)]
struct CommonLogArgs {
    /// Path to Arc config file (yaml/json). Used to locate logging.output.file.
    #[arg(long, default_value = "/etc/arc/config.yaml")]
    config: PathBuf,

    /// Override log file path directly (bypasses config).
    #[arg(long)]
    file: Option<PathBuf>,

    /// Filter: level (info|error|warn|debug). Applied client-side.
    #[arg(long)]
    level: Option<String>,

    /// Filter: route. Applied client-side.
    #[arg(long)]
    route: Option<String>,

    /// Filter: status (e.g. 502, 5xx, 4xx). Applied client-side.
    #[arg(long)]
    status: Option<String>,

    /// Filter: trace id. Applied client-side.
    #[arg(long = "trace-id")]
    trace_id: Option<String>,

    /// Filter: client ip. Applied client-side.
    #[arg(long = "client-ip")]
    client_ip: Option<String>,

    /// Filter: upstream. Applied client-side.
    #[arg(long)]
    upstream: Option<String>,
}

#[derive(Args, Debug)]
struct TailArgs {
    #[command(flatten)]
    common: CommonLogArgs,

    /// Only show logs within last duration (e.g. 5m, 1h)
    #[arg(long)]
    last: Option<String>,
}

#[derive(Args, Debug)]
struct QueryArgs {
    #[command(flatten)]
    common: CommonLogArgs,

    /// Query window (e.g. 1h, 5m)
    #[arg(long)]
    last: Option<String>,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("arc cli error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    match cli.cmd {
        Command::Logs(cmd) => match cmd {
            LogsCmd::Tail(args) => cmd_tail(args),
            LogsCmd::Query(args) => cmd_query(args),
        },
    }
}

fn cmd_tail(args: TailArgs) -> Result<(), CliError> {
    let file_path = resolve_log_file(&args.common)?;
    let cutoff = args
        .last
        .as_deref()
        .and_then(parse_duration)
        .map(|d| now_unix_nanos().saturating_sub(d.as_nanos() as i128));

    tail_follow(&file_path, &args.common, cutoff)
}

fn cmd_query(args: QueryArgs) -> Result<(), CliError> {
    let file_path = resolve_log_file(&args.common)?;
    let cutoff = args
        .last
        .as_deref()
        .and_then(parse_duration)
        .map(|d| now_unix_nanos().saturating_sub(d.as_nanos() as i128));

    query_file(&file_path, &args.common, cutoff)
}

fn resolve_log_file(args: &CommonLogArgs) -> Result<PathBuf, CliError> {
    if let Some(p) = args.file.clone() {
        return Ok(p);
    }
    let raw = read_to_string(&args.config)?;
    let v = parse_config_value(&raw, &args.config)?;
    let p = v
        .get("logging")
        .and_then(|x| x.get("output"))
        .and_then(|x| x.get("file"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| CliError::Config("missing logging.output.file".to_string()))?;
    Ok(PathBuf::from(p))
}

fn parse_config_value(raw: &str, path: &Path) -> Result<Value, CliError> {
    // Decide by extension; default yaml.
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("yaml")
        .to_ascii_lowercase();

    if ext == "json" {
        serde_json::from_str(raw).map_err(|e| CliError::Config(format!("parse json failed: {e}")))
    } else {
        let y: serde_yaml::Value = serde_yaml::from_str(raw)
            .map_err(|e| CliError::Config(format!("parse yaml failed: {e}")))?;
        // Convert yaml value -> json value
        let j = serde_json::to_value(y)
            .map_err(|e| CliError::Config(format!("yaml->json failed: {e}")))?;
        Ok(j)
    }
}

fn read_to_string(path: &Path) -> Result<String, CliError> {
    let mut f =
        File::open(path).map_err(|e| CliError::Io(format!("open config {}", path.display()), e))?;
    let mut s = String::new();
    f.read_to_string(&mut s)
        .map_err(|e| CliError::Io(format!("read config {}", path.display()), e))?;
    Ok(s)
}

fn tail_follow(
    path: &Path,
    f: &CommonLogArgs,
    cutoff_unix_nanos: Option<i128>,
) -> Result<(), CliError> {
    let mut file =
        File::open(path).map_err(|e| CliError::Io(format!("open log {}", path.display()), e))?;

    // Start near end for tail-like behavior: read last 4MB to find recent lines.
    let _ = file
        .seek(SeekFrom::End(-4 * 1024 * 1024))
        .or_else(|_| file.seek(SeekFrom::Start(0)));

    let mut reader = BufReader::new(file);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| CliError::Io("read log line".to_string(), e))?;

        if n == 0 {
            // EOF: sleep and try to reopen if rotated/truncated.
            thread::sleep(Duration::from_millis(200));
            // Re-open to follow rotations (tail -F semantics).
            let mut newf = match File::open(path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let _ = newf.seek(SeekFrom::Current(0));
            reader = BufReader::new(newf);
            continue;
        }

        if let Some(v) = parse_json_line(&line) {
            if !passes_cutoff(&v, cutoff_unix_nanos) {
                continue;
            }
            if matches_filters(&v, f) {
                print!("{}", line);
            }
        }
    }
}

fn query_file(
    path: &Path,
    f: &CommonLogArgs,
    cutoff_unix_nanos: Option<i128>,
) -> Result<(), CliError> {
    let file =
        File::open(path).map_err(|e| CliError::Io(format!("open log {}", path.display()), e))?;
    let reader = BufReader::new(file);

    for l in reader.lines() {
        let line = l.map_err(|e| CliError::Io("read log line".to_string(), e))?;
        if let Some(v) = parse_json_line(&line) {
            if !passes_cutoff(&v, cutoff_unix_nanos) {
                continue;
            }
            if matches_filters(&v, f) {
                println!("{line}");
            }
        }
    }
    Ok(())
}

fn parse_json_line(line: &str) -> Option<Value> {
    let s = line.trim_end_matches('\n').trim_end_matches('\r');
    if s.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(s).ok()
}

fn matches_filters(v: &Value, f: &CommonLogArgs) -> bool {
    if let Some(level) = f.level.as_deref() {
        if v.get("level").and_then(|x| x.as_str()).unwrap_or("") != level {
            return false;
        }
    }
    if let Some(route) = f.route.as_deref() {
        if v.get("route").and_then(|x| x.as_str()).unwrap_or("") != route {
            return false;
        }
    }
    if let Some(trace_id) = f.trace_id.as_deref() {
        if v.get("trace_id").and_then(|x| x.as_str()).unwrap_or("") != trace_id {
            return false;
        }
    }
    if let Some(client_ip) = f.client_ip.as_deref() {
        if v.get("client_ip").and_then(|x| x.as_str()).unwrap_or("") != client_ip {
            return false;
        }
    }
    if let Some(upstream) = f.upstream.as_deref() {
        if v.get("upstream").and_then(|x| x.as_str()).unwrap_or("") != upstream {
            return false;
        }
    }
    if let Some(status) = f.status.as_deref() {
        if !match_status(v.get("status"), status) {
            return false;
        }
    }
    true
}

fn match_status(v: Option<&Value>, pat: &str) -> bool {
    let Some(x) = v else { return false };
    let Some(code) = x.as_u64() else { return false };
    let p = pat.trim();
    if p.ends_with("xx") && p.len() == 3 {
        let c = p.as_bytes()[0];
        if !(b'1'..=b'5').contains(&c) {
            return false;
        }
        let base = (c - b'0') as u64 * 100;
        return code >= base && code < base + 100;
    }
    if let Ok(exact) = p.parse::<u64>() {
        return code == exact;
    }
    false
}

fn passes_cutoff(v: &Value, cutoff_unix_nanos: Option<i128>) -> bool {
    let Some(cutoff) = cutoff_unix_nanos else {
        return true;
    };
    // ts is RFC3339; we do a cheap parse to unix nanos.
    let ts = v.get("ts").and_then(|x| x.as_str()).unwrap_or("");
    match parse_rfc3339_to_unix_nanos(ts) {
        Some(n) => n >= cutoff,
        None => true, // if cannot parse, keep line
    }
}

fn parse_duration(s: &str) -> Option<Duration> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if t == "0" {
        return Some(Duration::from_secs(0));
    }
    let (num, unit) = split_num_unit(t)?;
    let n: u64 = num.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(n)),
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n.saturating_mul(60))),
        "h" => Some(Duration::from_secs(n.saturating_mul(3600))),
        _ => None,
    }
}

fn split_num_unit(s: &str) -> Option<(&str, &str)> {
    let mut idx = 0usize;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            idx = i + ch.len_utf8();
            continue;
        }
        idx = i;
        break;
    }
    if idx == 0 {
        return None;
    }
    let num = &s[..idx];
    let unit = s[idx..].trim();
    if unit.is_empty() {
        return Some((num, "s"));
    }
    Some((num, unit))
}

/// Very small RFC3339 nanos parser for `YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`.
/// If format differs, returns None.
fn parse_rfc3339_to_unix_nanos(ts: &str) -> Option<i128> {
    // We intentionally keep this strict & fast; logs always emitted in this format by arc-logging.
    if ts.len() < 20 {
        return None;
    }
    // Use timegm-like conversion via libc timegm if available.
    // Parse components.
    let year: i32 = ts.get(0..4)?.parse().ok()?;
    let mon: i32 = ts.get(5..7)?.parse().ok()?;
    let day: i32 = ts.get(8..10)?.parse().ok()?;
    let hour: i32 = ts.get(11..13)?.parse().ok()?;
    let min: i32 = ts.get(14..16)?.parse().ok()?;
    let sec: i32 = ts.get(17..19)?.parse().ok()?;

    let mut nanos: i32 = 0;
    if let Some(dot) = ts.find('.') {
        if let Some(z) = ts.rfind('Z') {
            let frac = &ts[dot + 1..z];
            if frac.len() == 9 && frac.chars().all(|c| c.is_ascii_digit()) {
                nanos = frac.parse().ok()?;
            }
        }
    }

    #[cfg(unix)]
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = mon - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour;
        tm.tm_min = min;
        tm.tm_sec = sec;
        // timegm converts tm in UTC to epoch seconds (GNU extension; on musl it exists as `timegm`).
        let secs = libc::timegm(&mut tm as *mut libc::tm);
        if secs < 0 {
            return None;
        }
        let unix_nanos = (secs as i128) * 1_000_000_000i128 + (nanos as i128);
        return Some(unix_nanos);
    }

    #[cfg(not(unix))]
    {
        let _ = (year, mon, day, hour, min, sec, nanos);
        None
    }
}

fn now_unix_nanos() -> i128 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
    (d.as_secs() as i128) * 1_000_000_000i128 + (d.subsec_nanos() as i128)
}

#[derive(thiserror::Error, Debug)]
enum CliError {
    #[error("io error: {0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("config error: {0}")]
    Config(String),
}
