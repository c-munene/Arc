# Arc Benchmark

This directory provides a reproducible benchmark workflow for Arc.
Use these scripts when you publish performance numbers so others can verify results.

## Layout

- `benchmark/scripts/run_h1_wrk_vs_nginx.sh` runs HTTP/1.1 tests with `wrk` against backend, Arc, and Nginx.
- `benchmark/scripts/run_h2_h2load_vs_nginx.sh` runs HTTP/2 tests with `h2load` against Arc and Nginx.
- `benchmark/scripts/parse_wrk.py` parses raw `wrk` output.
- `benchmark/scripts/parse_h2load.py` parses raw `h2load` output.
- `benchmark/backends/http_ok_backend.py` is a simple backend server for tests.
- `benchmark/templates/result_schema.json` defines the machine readable result format.
- `benchmark/templates/result_comparison.md` is a publication template.
- `benchmark/results/` stores run artifacts.

## Requirements

- Linux or WSL2
- `wrk`
- `h2load`
- `nginx`
- `python3`
- `curl`
- `openssl`

## Quick start

### HTTP/1.1

```bash
bash benchmark/scripts/run_h1_wrk_vs_nginx.sh
```

Default values:

- `RUNS=5`
- `THREADS=8`
- `CONNECTIONS=256`
- `DURATION=30s`
- `WARMUP=5s`
- `REQUIRE_ZERO_NON2XX=1`

### HTTP/2

```bash
bash benchmark/scripts/run_h2_h2load_vs_nginx.sh
```

Default values:

- `RUNS=5`
- `REQUESTS=20000`
- `CLIENTS=64`
- `STREAMS=20`
- `THREADS=2`
- `WARMUP_REQUESTS=1000`

If `h2load` is missing, install the nghttp2 client package.

## Output

Each run creates a timestamped folder such as:

`benchmark/results/h1_wrk_20260301_121530/`

Each folder contains:

- Raw benchmark output for every case and run
- `env.txt` with command line, tool versions, and commit
- `summary.json` for automation
- `summary.md` for human review

## Reproducibility checklist

- Use a fixed machine profile and kernel version
- Pin Arc and Nginx build versions
- Run at least five rounds with identical settings
- Use median values for comparison
- Keep raw output files with published claims

## Platform note

Arc data plane requires Linux capabilities such as `io_uring`.
Use WSL2 on Windows or a Linux host for benchmark runs.
