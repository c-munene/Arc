#!/usr/bin/env python3
import argparse
import http.client
import json
import threading
import time
import urllib.parse
from collections import defaultdict


def parse_targets(spec: str):
    targets = []
    for chunk in spec.split(";"):
        chunk = chunk.strip()
        if not chunk:
            continue
        if "," not in chunk:
            raise ValueError(f"invalid target spec '{chunk}', expected '<url>,<rps>'")
        u, r = chunk.rsplit(",", 1)
        url = u.strip()
        rps = float(r.strip())
        if rps < 0:
            raise ValueError(f"target rps must be >=0, got {rps}")
        p = urllib.parse.urlparse(url)
        if p.scheme != "http":
            raise ValueError(f"only http is supported, got '{url}'")
        host = p.hostname or "127.0.0.1"
        port = p.port or 80
        path = p.path or "/"
        if p.query:
            path += "?" + p.query
        targets.append(
            {
                "url": url,
                "host": host,
                "port": port,
                "path": path,
                "rps": rps,
            }
        )
    if not targets:
        raise ValueError("empty targets")
    return targets


class Recorder:
    def __init__(self, start_ts: float, duration_s: float, target_count: int):
        self.lock = threading.Lock()
        self.start_ts = start_ts
        self.duration_s = duration_s
        self.end_ts = start_ts + duration_s
        self.per_target = []
        self.timeline = []
        slots = int(duration_s) + 2
        for _ in range(target_count):
            self.per_target.append(
                {
                    "sent": 0,
                    "errors": 0,
                    "status": defaultdict(int),
                }
            )
            self.timeline.append(defaultdict(lambda: defaultdict(int)))
        self.global_timeline = defaultdict(lambda: defaultdict(int))

    def record(self, target_idx: int, status: int | None, ts: float):
        sec = int(max(0.0, ts - self.start_ts))
        with self.lock:
            t = self.per_target[target_idx]
            t["sent"] += 1
            if status is None:
                t["errors"] += 1
                self.timeline[target_idx][sec]["error"] += 1
                self.global_timeline[sec]["error"] += 1
                return
            k = str(status)
            t["status"][k] += 1
            self.timeline[target_idx][sec][k] += 1
            self.global_timeline[sec][k] += 1

    def to_json(self, targets):
        out = {
            "start_unix_s": self.start_ts,
            "duration_s": self.duration_s,
            "targets": [],
            "global": {},
            "timeline_global": [],
            "timeline_targets": [],
        }
        for i, t in enumerate(targets):
            rec = self.per_target[i]
            sent = rec["sent"]
            status = dict(sorted(rec["status"].items(), key=lambda x: int(x[0])))
            ok = sum(v for k, v in status.items() if 200 <= int(k) < 300)
            out["targets"].append(
                {
                    "url": t["url"],
                    "target_rps": t["rps"],
                    "sent": sent,
                    "achieved_rps": sent / self.duration_s if self.duration_s > 0 else 0.0,
                    "errors": rec["errors"],
                    "status": status,
                    "2xx": ok,
                    "429": status.get("429", 0),
                }
            )
            tl = []
            for sec in sorted(self.timeline[i].keys()):
                row = {"sec": sec}
                row.update(dict(self.timeline[i][sec]))
                tl.append(row)
            out["timeline_targets"].append(tl)

        g_status = defaultdict(int)
        g_sent = 0
        g_errors = 0
        for t in out["targets"]:
            g_sent += t["sent"]
            g_errors += t["errors"]
            for k, v in t["status"].items():
                g_status[k] += v
        g_2xx = sum(v for k, v in g_status.items() if 200 <= int(k) < 300)
        out["global"] = {
            "sent": g_sent,
            "achieved_rps": g_sent / self.duration_s if self.duration_s > 0 else 0.0,
            "errors": g_errors,
            "status": dict(sorted(g_status.items(), key=lambda x: int(x[0]))),
            "2xx": g_2xx,
            "429": g_status.get("429", 0),
        }
        for sec in sorted(self.global_timeline.keys()):
            row = {"sec": sec}
            row.update(dict(self.global_timeline[sec]))
            out["timeline_global"].append(row)
        return out


def worker_loop(
    target_idx: int,
    host: str,
    port: int,
    path: str,
    rate: float,
    start_ts: float,
    duration_s: float,
    recorder: Recorder,
    timeout_s: float,
):
    if rate <= 0:
        return
    interval = 1.0 / rate
    end_ts = start_ts + duration_s
    next_ts = start_ts
    conn = None
    while True:
        now = time.monotonic()
        if now >= end_ts:
            break
        if next_ts > now:
            time.sleep(next_ts - now)
        else:
            # Avoid long drift when the sender falls behind.
            missed = int((now - next_ts) / interval)
            if missed > 0:
                next_ts += missed * interval
        next_ts += interval
        status = None
        try:
            if conn is None:
                conn = http.client.HTTPConnection(host, port, timeout=timeout_s)
            conn.request("GET", path, headers={"Connection": "keep-alive"})
            resp = conn.getresponse()
            status = int(resp.status)
            _ = resp.read()
        except Exception:
            status = None
            try:
                if conn is not None:
                    conn.close()
            except Exception:
                pass
            conn = None
        recorder.record(target_idx, status, time.monotonic())
    try:
        if conn is not None:
            conn.close()
    except Exception:
        pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", required=True, help="'<url>,<rps>;<url>,<rps>'")
    ap.add_argument("--duration", type=float, required=True)
    ap.add_argument("--workers-per-target", type=int, default=4)
    ap.add_argument("--timeout", type=float, default=3.0)
    ap.add_argument("--output", required=True)
    args = ap.parse_args()

    targets = parse_targets(args.targets)
    workers = max(1, args.workers_per_target)
    start_ts = time.monotonic() + 0.6
    rec = Recorder(start_ts, args.duration, len(targets))
    threads = []
    for idx, t in enumerate(targets):
        total = t["rps"]
        per = total / workers
        for _ in range(workers):
            th = threading.Thread(
                target=worker_loop,
                args=(
                    idx,
                    t["host"],
                    t["port"],
                    t["path"],
                    per,
                    start_ts,
                    args.duration,
                    rec,
                    args.timeout,
                ),
                daemon=True,
            )
            threads.append(th)

    for th in threads:
        th.start()
    for th in threads:
        th.join()

    out = rec.to_json(targets)
    with open(args.output, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)


if __name__ == "__main__":
    main()
