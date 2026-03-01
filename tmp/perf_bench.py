import argparse
import asyncio
import json
import math
import time
from typing import Dict, List

import aiohttp


def percentile(sorted_values: List[float], p: float) -> float:
    if not sorted_values:
        return 0.0
    if p <= 0:
        return sorted_values[0]
    if p >= 100:
        return sorted_values[-1]
    k = (len(sorted_values) - 1) * (p / 100.0)
    f = math.floor(k)
    c = math.ceil(k)
    if f == c:
        return sorted_values[int(k)]
    return sorted_values[f] * (c - k) + sorted_values[c] * (k - f)


async def worker(
    session: aiohttp.ClientSession,
    url: str,
    headers: Dict[str, str],
    deadline: float,
    latencies_ms: List[float],
    counters: Dict[str, int],
) -> None:
    while time.perf_counter() < deadline:
        start = time.perf_counter()
        try:
            async with session.get(url, headers=headers) as resp:
                await resp.read()
                elapsed = (time.perf_counter() - start) * 1000.0
                latencies_ms.append(elapsed)
                if 200 <= resp.status < 400:
                    counters['ok'] += 1
                else:
                    counters['non_2xx_3xx'] += 1
        except Exception:
            elapsed = (time.perf_counter() - start) * 1000.0
            latencies_ms.append(elapsed)
            counters['errors'] += 1


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--url', required=True)
    parser.add_argument('--duration', type=float, default=15.0)
    parser.add_argument('--concurrency', type=int, default=128)
    parser.add_argument('--timeout', type=float, default=10.0)
    parser.add_argument('--host-header', default='')
    args = parser.parse_args()

    headers = {}
    if args.host_header:
        headers['Host'] = args.host_header

    latencies_ms: List[float] = []
    counters = {'ok': 0, 'non_2xx_3xx': 0, 'errors': 0}

    timeout = aiohttp.ClientTimeout(total=args.timeout)
    connector = aiohttp.TCPConnector(limit=0, ssl=False)

    start = time.perf_counter()
    deadline = start + args.duration

    async with aiohttp.ClientSession(timeout=timeout, connector=connector) as session:
        tasks = [
            asyncio.create_task(worker(session, args.url, headers, deadline, latencies_ms, counters))
            for _ in range(args.concurrency)
        ]
        await asyncio.gather(*tasks)

    end = time.perf_counter()
    total_time = end - start
    total = counters['ok'] + counters['non_2xx_3xx'] + counters['errors']
    latencies_ms.sort()

    result = {
        'url': args.url,
        'duration_sec': total_time,
        'concurrency': args.concurrency,
        'total_requests': total,
        'ok': counters['ok'],
        'non_2xx_3xx': counters['non_2xx_3xx'],
        'errors': counters['errors'],
        'rps': (total / total_time) if total_time > 0 else 0.0,
        'latency_ms': {
            'avg': (sum(latencies_ms) / len(latencies_ms)) if latencies_ms else 0.0,
            'p50': percentile(latencies_ms, 50),
            'p95': percentile(latencies_ms, 95),
            'p99': percentile(latencies_ms, 99),
            'max': latencies_ms[-1] if latencies_ms else 0.0,
        },
    }
    print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    asyncio.run(main())
