use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aya::programs::{links::FdLink, Xdp, XdpFlags};
use aya::Ebpf;

const DEFAULT_PIN_BASE: &str = "/sys/fs/bpf/arc";
const DEFAULT_OBJ: &str = "target/bpfel-unknown-none/release/arc_xdp";

fn main() {
    if let Err(e) = run() {
        eprintln!("arc-xdp-loader error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return Err(usage());
    }

    let cmd = args.remove(0);
    match cmd.as_str() {
        "attach" => {
            let iface = take_opt(&mut args, "--iface").ok_or_else(usage)?;
            let mode = take_opt(&mut args, "--mode").unwrap_or_else(|| "generic".to_string());
            let hold_secs = take_opt(&mut args, "--hold-secs")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let pin_base = resolve_pin_base(take_opt(&mut args, "--pin-base"));
            attach(&iface, &mode, hold_secs, pin_base.as_str())
        }
        "detach" => {
            let iface = take_opt(&mut args, "--iface").ok_or_else(usage)?;
            let pin_base = resolve_pin_base(take_opt(&mut args, "--pin-base"));
            detach(&iface, pin_base.as_str())
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: arc-xdp-loader attach --iface <ifname> [--mode driver|generic] [--pin-base <path>] | detach --iface <ifname> [--pin-base <path>]".to_string()
}

fn take_opt(args: &mut Vec<String>, key: &str) -> Option<String> {
    let idx = args.iter().position(|v| v == key)?;
    if idx + 1 >= args.len() {
        return None;
    }
    Some(args.remove(idx + 1))
}

fn resolve_pin_base(cli: Option<String>) -> String {
    if let Some(v) = cli {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(v) = env::var("ARC_XDP_PIN_BASE") {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    DEFAULT_PIN_BASE.to_string()
}

fn pin_progs(pin_base: &str) -> String {
    format!("{}/progs", pin_base.trim_end_matches('/'))
}

fn pin_link(pin_base: &str) -> String {
    format!("{}/progs/arc_xdp_link", pin_base.trim_end_matches('/'))
}

fn attach(iface: &str, mode: &str, hold_secs: u64, pin_base: &str) -> Result<(), String> {
    let pin_progs = pin_progs(pin_base);
    let pin_link = pin_link(pin_base);
    ensure_dir(pin_base)?;
    ensure_dir(&pin_progs)?;

    // Reset existing XDP link on this interface first.
    let _ = Command::new("ip")
        .args(["link", "set", "dev", iface, "xdp", "off"])
        .status();
    let _ = fs::remove_file(&pin_link);

    let obj = resolve_obj_path();
    if !obj.exists() {
        return Err(format!("bpf object not found: {}", obj.display()));
    }

    let mut bpf =
        Ebpf::load_file(&obj).map_err(|e| format!("load {} failed: {e}", obj.display()))?;

    pin_maps(&mut bpf, pin_base)?;

    let flags = match mode {
        "driver" => XdpFlags::DRV_MODE,
        "generic" => XdpFlags::SKB_MODE,
        "tc" => {
            return Err(
                "mode=tc is not implemented in arc-xdp-loader; please use arc-tc-loader"
                    .to_string(),
            )
        }
        other => return Err(format!("invalid mode: {other}")),
    };

    let program = bpf
        .program_mut("arc_xdp")
        .ok_or_else(|| "program arc_xdp not found".to_string())?;
    let program: &mut Xdp = program
        .try_into()
        .map_err(|e| format!("program cast to Xdp failed: {e}"))?;

    program
        .load()
        .map_err(|e| format!("xdp load failed: {e}"))?;
    let link_id = program
        .attach(iface, flags)
        .map_err(|e| format!("xdp attach failed on {iface} ({mode}): {e}"))?;

    let link = program
        .take_link(link_id)
        .map_err(|e| format!("take xdp link failed: {e}"))?;
    let fd_link: FdLink = link
        .try_into()
        .map_err(|e| format!("xdp link is not fd-link (kernel<5.9?): {e}"))?;
    let _pinned = fd_link
        .pin(&pin_link)
        .map_err(|e| format!("pin xdp link failed: {e}"))?;

    println!(
        "attached: iface={} mode={} obj={} hold_secs={} pinned_link={}",
        iface,
        mode,
        obj.display(),
        hold_secs,
        pin_link
    );
    if hold_secs > 0 {
        std::thread::sleep(std::time::Duration::from_secs(hold_secs));
    }
    Ok(())
}

fn detach(iface: &str, pin_base: &str) -> Result<(), String> {
    let pin_link = pin_link(pin_base);
    let st = Command::new("ip")
        .args(["link", "set", "dev", iface, "xdp", "off"])
        .status()
        .map_err(|e| format!("ip link xdp off failed: {e}"))?;
    if !st.success() {
        return Err(format!("ip link xdp off returned non-zero: {st}"));
    }
    let _ = fs::remove_file(pin_link);
    println!("detached: iface={iface}");
    Ok(())
}

fn resolve_obj_path() -> PathBuf {
    if let Ok(p) = env::var("ARC_XDP_OBJ") {
        return PathBuf::from(p);
    }
    PathBuf::from(DEFAULT_OBJ)
}

fn pin_maps(bpf: &mut Ebpf, pin_base: &str) -> Result<(), String> {
    let mut pinned = 0usize;
    for (name, map) in bpf.maps_mut() {
        let pin_name = logical_map_name(name);
        let pin_path = Path::new(pin_base).join(pin_name);
        let _ = fs::remove_file(&pin_path);
        map.pin(&pin_path)
            .map_err(|e| format!("pin map {} -> {} failed: {e}", name, pin_path.display()))?;
        pinned += 1;
    }
    if pinned == 0 {
        return Err("no maps were pinned".to_string());
    }
    Ok(())
}

fn logical_map_name(raw: &str) -> String {
    if let Some(stripped) = raw.strip_prefix("arc_") {
        stripped.to_string()
    } else {
        raw.to_string()
    }
}

fn ensure_dir(path: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir {} failed: {e}", path))
}
