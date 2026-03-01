#!/usr/bin/env python3
import argparse
import glob
import json
import re
import statistics
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Dict, List, Tuple


PAT_RPS = re.compile(r"Requests/sec:\s+([0-9.]+)")
PAT_LAT = re.compile(
    r"Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)"
)
PAT_NON_2XX = re.compile(r"Non-2xx or 3xx responses:\s+(\d+)")
PAT_SOCKET = re.compile(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)")
PAT_REQ = re.compile(r"(\d+)\s+requests in\s+([0-9.]+)([a-zA-Z]+)")


def to_ms(value: float, unit: str) -> float:
    u = unit.lower()
    if u == "us":
        return value / 1000.0
    if u == "ms":
        return value
    if u == "s":
        return value * 1000.0
    return value


@dataclass
class Run:
    file: str
    requests_per_sec: float
    latency_avg_ms: float
    latency_stdev_ms: float
    latency_max_ms: float
    total_requests: int
    duration_sec: float
    non_2xx_3xx: int
    socket_errors: int
    effective_2xx_3xx_rps: float


def parse_file(path: Path) -> Run:
    text = path.read_text(encoding="utf-8", errors="ignore")

    rps_match = PAT_RPS.search(text)
    lat_match = PAT_LAT.search(text)
    req_match = PAT_REQ.search(text)
    non_2xx_match = PAT_NON_2XX.search(text)
    socket_match = PAT_SOCKET.search(text)

    rps = float(rps_match.group(1)) if rps_match else 0.0
    lat_avg = to_ms(float(lat_match.group(1)), lat_match.group(2)) if lat_match else 0.0
    lat_stdev = to_ms(float(lat_match.group(3)), lat_match.group(4)) if lat_match else 0.0
    lat_max = to_ms(float(lat_match.group(5)), lat_match.group(6)) if lat_match else 0.0
    req = int(req_match.group(1)) if req_match else 0
    duration_sec = 0.0
    if req_match:
        raw_dur = float(req_match.group(2))
        raw_unit = req_match.group(3).lower()
        if raw_unit == "s":
            duration_sec = raw_dur
        elif raw_unit == "ms":
            duration_sec = raw_dur / 1000.0
        elif raw_unit == "us":
            duration_sec = raw_dur / 1_000_000.0
        elif raw_unit == "m":
            duration_sec = raw_dur * 60.0

    non_2xx = int(non_2xx_match.group(1)) if non_2xx_match else 0
    socket_errors = sum(int(v) for v in socket_match.groups()) if socket_match else 0
    success = max(req - non_2xx, 0)
    effective_rps = (success / duration_sec) if duration_sec > 0 else 0.0

    return Run(
        file=str(path),
        requests_per_sec=rps,
        latency_avg_ms=lat_avg,
        latency_stdev_ms=lat_stdev,
        latency_max_ms=lat_max,
        total_requests=req,
        duration_sec=duration_sec,
        non_2xx_3xx=non_2xx,
        socket_errors=socket_errors,
        effective_2xx_3xx_rps=effective_rps,
    )


def parse_case_spec(spec: str) -> Tuple[str, str]:
    if ":" not in spec:
        raise ValueError(f"invalid --case '{spec}', expected 'name:glob'")
    name, pattern = spec.split(":", 1)
    name = name.strip()
    pattern = pattern.strip()
    if not name or not pattern:
        raise ValueError(f"invalid --case '{spec}', empty name or pattern")
    return name, pattern


def agg_float(values: List[float]) -> Dict[str, float]:
    if not values:
        return {"mean": 0.0, "median": 0.0, "min": 0.0, "max": 0.0}
    return {
        "mean": statistics.fmean(values),
        "median": statistics.median(values),
        "min": min(values),
        "max": max(values),
    }


def agg_int(values: List[int]) -> Dict[str, float]:
    if not values:
        return {"sum": 0, "mean": 0.0, "median": 0.0}
    return {
        "sum": int(sum(values)),
        "mean": statistics.fmean(values),
        "median": statistics.median(values),
    }


def build_markdown(summary: Dict) -> str:
    lines = []
    lines.append("# wrk benchmark summary")
    lines.append("")
    lines.append("| case | runs | rps(median) | effective_2xx_rps(median) | latency_avg_ms(median) | non_2xx(sum) | socket_errors(sum) |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|")
    for name, item in summary["cases"].items():
        lines.append(
            f"| {name} | {item['run_count']} | "
            f"{item['requests_per_sec']['median']:.2f} | "
            f"{item['effective_2xx_3xx_rps']['median']:.2f} | "
            f"{item['latency_avg_ms']['median']:.3f} | "
            f"{int(item['non_2xx_3xx']['sum'])} | "
            f"{int(item['socket_errors']['sum'])} |"
        )
    if "compare" in summary:
        lines.append("")
        lines.append("## compare")
        for k, v in summary["compare"].items():
            if isinstance(v, float):
                lines.append(f"- {k}: {v:.4f}")
            else:
                lines.append(f"- {k}: {v}")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description="Parse wrk outputs into summary JSON")
    parser.add_argument("--case", action="append", required=True, help="name:glob")
    parser.add_argument("--output", required=True)
    parser.add_argument("--markdown", default="")
    args = parser.parse_args()

    cases: Dict[str, Dict] = {}
    for spec in args.case:
        name, pattern = parse_case_spec(spec)
        paths = sorted(Path(p) for p in glob.glob(pattern))
        if not paths:
            raise SystemExit(f"no files matched for case '{name}': {pattern}")
        runs = [parse_file(p) for p in paths]
        cases[name] = {
            "run_count": len(runs),
            "runs": [asdict(r) for r in runs],
            "requests_per_sec": agg_float([r.requests_per_sec for r in runs]),
            "effective_2xx_3xx_rps": agg_float([r.effective_2xx_3xx_rps for r in runs]),
            "latency_avg_ms": agg_float([r.latency_avg_ms for r in runs]),
            "latency_stdev_ms": agg_float([r.latency_stdev_ms for r in runs]),
            "latency_max_ms": agg_float([r.latency_max_ms for r in runs]),
            "total_requests": agg_int([r.total_requests for r in runs]),
            "duration_sec": agg_float([r.duration_sec for r in runs]),
            "non_2xx_3xx": agg_int([r.non_2xx_3xx for r in runs]),
            "socket_errors": agg_int([r.socket_errors for r in runs]),
        }

    summary = {"tool": "wrk", "cases": cases}

    if "arc" in cases and "nginx" in cases:
        arc_rps = cases["arc"]["requests_per_sec"]["median"]
        nginx_rps = cases["nginx"]["requests_per_sec"]["median"]
        arc_eff_rps = cases["arc"]["effective_2xx_3xx_rps"]["median"]
        nginx_eff_rps = cases["nginx"]["effective_2xx_3xx_rps"]["median"]
        summary["compare"] = {
            "arc_vs_nginx_rps_ratio_median": (arc_rps / nginx_rps) if nginx_rps else 0.0,
            "arc_vs_nginx_rps_gap_pct_median": ((nginx_rps - arc_rps) / nginx_rps * 100.0)
            if nginx_rps
            else 0.0,
            "arc_vs_nginx_effective_2xx_rps_ratio_median": (arc_eff_rps / nginx_eff_rps)
            if nginx_eff_rps
            else 0.0,
            "arc_vs_nginx_effective_2xx_rps_gap_pct_median": (
                (nginx_eff_rps - arc_eff_rps) / nginx_eff_rps * 100.0
            )
            if nginx_eff_rps
            else 0.0,
        }

    out_path = Path(args.output)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")

    if args.markdown:
        md_path = Path(args.markdown)
        md_path.parent.mkdir(parents=True, exist_ok=True)
        md_path.write_text(build_markdown(summary), encoding="utf-8")

    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
