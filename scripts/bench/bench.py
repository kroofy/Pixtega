#!/usr/bin/env python3
"""End-to-end benchmark for pixtega.

Measures, per scenario (one fresh server process per scenario so peak RSS is
attributable):
  - sequential request latency (mean / p50 / min over N requests)
  - throughput at fixed concurrency (requests per second)
  - server peak RSS (VmHWM) and final RSS after the scenario
  - response body size (sanity check that behavior did not change)

Usage:
  scripts/bench/bench.py run --binary target/release/pixtega --label baseline
  scripts/bench/bench.py compare bench/results/baseline.json bench/results/opt.json
"""

import argparse
import json
import os
import signal
import statistics
import subprocess
import sys
import threading
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CONFIG = os.path.join(ROOT, "scripts", "bench", "bench-config.toml")
FIXTURES = os.path.join(ROOT, "bench", "fixtures")
RESULTS_DIR = os.path.join(ROOT, "bench", "results")
BASE = "http://127.0.0.1:8090"

# (name, path, sequential_n, concurrent_n)
SCENARIOS = [
    ("small-jpg->w320.webp",        "/images/fs/small.jpg/w320.webp",          30, 60),
    ("medium-webp->w640.webp",      "/images/fs/medium.webp/w640.webp",        15, 24),
    ("icc-jpg->w1280.webp",         "/images/fs/icc.jpg/w1280.webp",           15, 24),
    ("complex-12MP-jpg->w1280.webp","/images/fs/complex.jpg/w1280.webp",       10, 16),
    ("large-32MP-jpg->w1280.webp",  "/images/fs/large-photo.jpg/w1280.webp",   10, 16),
    ("large-32MP-jpg->w1280.jpeg",  "/images/fs/large-photo.jpg/w1280.jpeg",   10, 16),
    ("large-32MP-jpg->w640.avif",   "/images/fs/large-photo.jpg/w640.avif",     6,  8),
    ("alpha-6MP-png->w1280.webp",   "/images/fs/large-alpha.png/w1280.webp",   10, 16),
    ("alpha-6MP-png->w1280.jpeg",   "/images/fs/large-alpha.png/w1280.jpeg",   10, 16),
    ("http-large-32MP->w1280.webp", "/images/http/large-photo.jpg/w1280.webp", 10, 16),
    ("http-small->w320.webp",       "/images/http/small.jpg/w320.webp",        30, 60),
]

CONCURRENCY = 4


def fetch(path):
    start = time.perf_counter()
    with urllib.request.urlopen(BASE + path) as resp:
        body = resp.read()
        status = resp.status
    elapsed = time.perf_counter() - start
    if status != 200:
        raise RuntimeError(f"{path}: HTTP {status}")
    return elapsed, len(body)


def read_proc_status(pid):
    fields = {}
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith(("VmHWM:", "VmRSS:")):
                key, value = line.split(":", 1)
                fields[key] = int(value.strip().split()[0])  # kB
    return fields


def start_server(binary):
    proc = subprocess.Popen(
        [binary, CONFIG],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        cwd=ROOT,
    )
    deadline = time.time() + 15
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("server exited during startup")
        if '"event":"listening"' in line.replace(" ", ""):
            break
    else:
        raise RuntimeError("server did not report listening")
    # Drain stdout in the background so the pipe never fills up.
    threading.Thread(target=proc.stdout.read, daemon=True).start()
    return proc


def run_scenario(binary, name, path, seq_n, conc_n):
    proc = start_server(binary)
    try:
        for _ in range(3):  # warmup
            fetch(path)

        latencies = []
        body_len = None
        for _ in range(seq_n):
            elapsed, size = fetch(path)
            latencies.append(elapsed)
            body_len = size

        errors = []
        done = []
        lock = threading.Lock()
        remaining = [conc_n]

        def worker():
            while True:
                with lock:
                    if remaining[0] <= 0:
                        return
                    remaining[0] -= 1
                try:
                    done.append(fetch(path))
                except Exception as e:  # noqa: BLE001
                    errors.append(str(e))

        wall_start = time.perf_counter()
        threads = [threading.Thread(target=worker) for _ in range(CONCURRENCY)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        wall = time.perf_counter() - wall_start
        if errors:
            raise RuntimeError(f"{name}: concurrent errors: {errors[:3]}")

        mem = read_proc_status(proc.pid)
        return {
            "name": name,
            "path": path,
            "seq_n": seq_n,
            "latency_mean_ms": statistics.mean(latencies) * 1000,
            "latency_p50_ms": statistics.median(latencies) * 1000,
            "latency_min_ms": min(latencies) * 1000,
            "conc_n": conc_n,
            "concurrency": CONCURRENCY,
            "throughput_rps": conc_n / wall,
            "body_bytes": body_len,
            "peak_rss_kb": mem.get("VmHWM"),
            "final_rss_kb": mem.get("VmRSS"),
        }
    finally:
        proc.send_signal(signal.SIGKILL)
        proc.wait()


def cmd_run(args):
    fixture_server = subprocess.Popen(
        [sys.executable, "-m", "http.server", "8091", "-d", FIXTURES],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(0.5)
    results = []
    try:
        for name, path, seq_n, conc_n in SCENARIOS:
            print(f"== {name}", file=sys.stderr, flush=True)
            results.append(run_scenario(args.binary, name, path, seq_n, conc_n))
    finally:
        fixture_server.kill()
        fixture_server.wait()

    os.makedirs(RESULTS_DIR, exist_ok=True)
    out_path = os.path.join(RESULTS_DIR, f"{args.label}.json")
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"wrote {out_path}", file=sys.stderr)
    print_table(results)


def print_table(results):
    header = f"{'scenario':32} {'mean ms':>9} {'p50 ms':>9} {'min ms':>9} {'rps@4':>7} {'peakRSS MB':>10} {'body B':>9}"
    print(header)
    print("-" * len(header))
    for r in results:
        print(
            f"{r['name']:32} {r['latency_mean_ms']:9.1f} {r['latency_p50_ms']:9.1f} "
            f"{r['latency_min_ms']:9.1f} {r['throughput_rps']:7.2f} "
            f"{(r['peak_rss_kb'] or 0) / 1024:10.1f} {r['body_bytes']:9d}"
        )


def cmd_compare(args):
    with open(args.a) as f:
        a = {r["name"]: r for r in json.load(f)}
    with open(args.b) as f:
        b = {r["name"]: r for r in json.load(f)}
    la, lb = os.path.basename(args.a), os.path.basename(args.b)
    header = (
        f"{'scenario':32} {'p50 A':>9} {'p50 B':>9} {'Δp50':>7} "
        f"{'rps A':>7} {'rps B':>7} {'ΔRSS MB':>8} {'ΔbodyB':>8}"
    )
    print(f"A={la}  B={lb}")
    print(header)
    print("-" * len(header))
    for name in a:
        if name not in b:
            continue
        ra, rb = a[name], b[name]
        dp = (rb["latency_p50_ms"] - ra["latency_p50_ms"]) / ra["latency_p50_ms"] * 100
        drss = ((rb["peak_rss_kb"] or 0) - (ra["peak_rss_kb"] or 0)) / 1024
        dbody = rb["body_bytes"] - ra["body_bytes"]
        print(
            f"{name:32} {ra['latency_p50_ms']:9.1f} {rb['latency_p50_ms']:9.1f} {dp:+6.1f}% "
            f"{ra['throughput_rps']:7.2f} {rb['throughput_rps']:7.2f} {drss:+8.1f} {dbody:+8d}"
        )


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("--binary", default=os.path.join(ROOT, "target", "release", "pixtega"))
    run.add_argument("--label", required=True)
    run.set_defaults(func=cmd_run)
    cmp_ = sub.add_parser("compare")
    cmp_.add_argument("a")
    cmp_.add_argument("b")
    cmp_.set_defaults(func=cmd_compare)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
