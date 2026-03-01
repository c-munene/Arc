#!/usr/bin/env python3
import argparse
import glob
import json
import re
import statistics
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Dict, List, Tuple


PAT_RPS = re.compile(r"finished in [^,]+,\s*([0-9.]+)\s*req/s", re.IGNORECASE)
PAT_REQ = re.compile(
    r"requests:\s*([0-9]+)\s*total,\s*([0-9]+)\s*started,\s*([0-9]+)\s*done,\s*([0-9]+)\s*succeeded,\s*([0-9]+)\s*failed,\s*([0-9]+)\s*errored,\s*([0-9]+)\s*timeout",
    re.IGNORECASE,
)


@dataclass
class Run:
    file: str
    requests_per_sec: float
    total: int
    started: int
    done: int
    succeeded: int
    failed: int
    errored: int
    timeout: int


def parse_file(path: Path) -> Run:
    text = path.read_text(encoding="utf-8", errors="ignore")
    rps_match = PAT_RPS.search(text)
    req_match = PAT_REQ.search(text)

    rps = float(rps_match.group(1)) if rps_match else 0.0
    if req_match:
        total, started, done, succeeded, failed, errored, timeout = [int(v) for v in req_match.groups()]
    else:
        total = started = done = succeeded = failed = errored = timeout = 0

    return Run(
        file=str(path),
        requests_per_sec=rps,
        total=total,
        started=started,
        done=done,
        succeeded=succeeded,
        failed=failed,
        errored=errored,
        timeout=timeout,
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
    lines.append("# h2load benchmark summary")
    lines.append("")
    lines.append("| case | runs | rps(median) | succeeded(sum) | failed(sum) | errored(sum) | timeout(sum) |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|")
    for name, item in summary["cases"].items():
        lines.append(
            f"| {name} | {item['run_count']} | "
            f"{item['requests_per_sec']['median']:.2f} | "
            f"{int(item['succeeded']['sum'])} | "
            f"{int(item['failed']['sum'])} | "
            f"{int(item['errored']['sum'])} | "
            f"{int(item['timeout']['sum'])} |"
        )
    if "compare" in summary:
        lines.append("")
        lines.append("## compare")
        for k, v in summary["compare"].items():
            lines.append(f"- {k}: {v:.4f}")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description="Parse h2load outputs into summary JSON")
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
            "total": agg_int([r.total for r in runs]),
            "started": agg_int([r.started for r in runs]),
            "done": agg_int([r.done for r in runs]),
            "succeeded": agg_int([r.succeeded for r in runs]),
            "failed": agg_int([r.failed for r in runs]),
            "errored": agg_int([r.errored for r in runs]),
            "timeout": agg_int([r.timeout for r in runs]),
        }

    summary = {"tool": "h2load", "cases": cases}
    if "arc" in cases and "nginx" in cases:
        arc_rps = cases["arc"]["requests_per_sec"]["median"]
        nginx_rps = cases["nginx"]["requests_per_sec"]["median"]
        summary["compare"] = {
            "arc_vs_nginx_rps_ratio_median": (arc_rps / nginx_rps) if nginx_rps else 0.0,
            "arc_vs_nginx_rps_gap_pct_median": ((nginx_rps - arc_rps) / nginx_rps * 100.0)
            if nginx_rps
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

