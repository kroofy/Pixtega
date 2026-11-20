#!/usr/bin/env python3
"""Benchmark a pixtega binary over the benchmark corpus.

    bench.py run [--label NAME] [--only TEXT]    measure; JSON on stdout
    bench.py report A.json [B.json]              one run, or B against A

`run` is meant to run inside the benchmark image, next to the binary and
the corpus (scripts/bench/bench.sh builds the image and runs it). It
measures:

  matrix      every scenario on its own fresh process, so its peak memory
              is its own: startup, the first request, the fastest and the
              median of the next few, CPU time per request, peak memory,
              and the size and hash of the response
  throughput  one small derivation from 8 connections for a few seconds
  burst       a shuffled mix of the 9.4 MP scenarios from 8 connections,
              then memory once the service has been idle long enough to
              hand freed memory back

A scenario is a source image, an output width, and an output format. The
matrix crosses four kinds of content with four source formats, three
output formats, and three widths, then varies the source size and the
source encoding (see gen-corpus.sh), and fetches a few sources over HTTP
instead of from disk.

Wall-clock numbers depend on the machine and on what else it is doing;
CPU time per request much less so. Peak memory repeats to within a few
megabytes for most scenarios, but a few move by a fifth from one run to
the next, so read it across a group before trusting a single scenario.
The response hashes do not vary at all: `report A B` says how many
responses are byte-identical. The service starts with a warm page cache;
coldstart.sh measures a cold one.
"""

import argparse
import collections
import hashlib
import http.client
import json
import os
import platform
import random
import statistics
import subprocess
import sys
import tempfile
import threading
import time

PORT = 8090
HTTP_FIXTURE_PORT = 8091
CONCURRENCY = 8
THROUGHPUT_SECONDS = 3
BURST_ROUNDS = 3
# Longer than the service's idle reclaim delay (IDLE_RECLAIM_DELAY in
# src/app.rs), so the settled memory is read after the reclaim has run.
SETTLE_SECONDS = 7
CLOCK_TICKS = os.sysconf("SC_CLK_TCK")

FORMATS = ["webp", "avif", "jpeg"]

# Limits are generous so that nothing is refused or times out: the
# benchmark measures what a derivation costs, not when it is cut short.
CONFIG = """
listen_address = "127.0.0.1:{port}"
path_prefix = "/images"
allowed_widths = [320, 1280, 1920, 3840]
max_download_bytes = 104857600
max_source_megapixels = 100
download_timeout_ms = 30000
request_timeout_ms = 300000
max_redirects = 3
max_concurrent_derivations = {concurrency}
success_ttl_seconds = 3600
not_found_ttl_seconds = 60

[formats.webp]
default_quality = 82
allowed_qualities = [60, 72, 90]

[formats.avif]
default_quality = 55
allowed_qualities = [40, 65]

[formats.jpeg]
default_quality = 85
allowed_qualities = [70, 92]

[[sources]]
mount = "fs"
key_prefix = ""
transport = "filesystem"
root = "{corpus}"

[[sources]]
mount = "http"
key_prefix = ""
transport = "http"
base_url = "http://127.0.0.1:{http_port}"
allow_private_destinations = true
"""


def scenarios():
    """Every scenario, in the order it runs."""

    def scenario(group, source, width, fmt, mount="fs"):
        stem, extension = source.rsplit(".", 1)
        content, _, size = stem.partition("-")
        return {
            "id": f"{mount}/{source} w{width}.{fmt}",
            "path": f"/images/{mount}/{source}/w{width}.{fmt}",
            "group": group,
            "content": content,
            "size": size,
            "source": source,
            "source_format": extension,
            "width": width,
            "format": fmt,
        }

    out = []
    for content in ["photo", "smooth", "graphic", "noise"]:
        for extension in ["jpg", "png", "webp", "avif"]:
            for fmt in FORMATS:
                for width in [320, 1280, 1920]:
                    out.append(scenario("grid", f"{content}-9.4mp.{extension}", width, fmt))
    for size in ["0.4mp", "1.6mp", "38mp"]:
        for extension in ["jpg", "png", "webp", "avif"]:
            for fmt in FORMATS:
                # Narrower sources are served at their own width: the
                # service never enlarges.
                for width in [320, 1920]:
                    out.append(scenario("size", f"photo-{size}.{extension}", width, fmt))
    for size in ["9.4mp", "38mp"]:
        for fmt in FORMATS:
            out.append(scenario("size", f"photo-{size}.jpg", 3840, fmt))
    for source in [
        "var-progressive.jpg", "var-420.jpg", "var-gray.jpg", "var-cmyk.jpg",
        "var-rotated.jpg", "var-interlaced.png", "var-palette.png", "var-16bit.png",
        "var-alpha.png", "var-lossless.webp", "var-alpha.webp", "var-alpha.avif",
    ]:  # fmt: skip
        for fmt in FORMATS:
            out.append(scenario("variant", source, 1280, fmt))
    for source in ["photo-0.4mp.jpg", "photo-9.4mp.jpg", "photo-38mp.png"]:
        out.append(scenario("http", source, 1280, "webp", mount="http"))
    return out


class Service:
    """One pixtega process, started for a measurement and killed after it."""

    def __init__(self, binary, config_path):
        spawned = time.perf_counter()
        self.process = subprocess.Popen(
            [binary, config_path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
        )
        for line in self.process.stdout:
            if '"event":"listening"' in line.replace(" ", ""):
                break
        else:
            raise RuntimeError("the service exited before it was listening")
        self.startup_ms = (time.perf_counter() - spawned) * 1000
        # Keep reading the request log so the pipe never fills.
        threading.Thread(target=self.process.stdout.read, daemon=True).start()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.process.kill()
        self.process.wait()

    def memory_mb(self):
        """Resident memory now (`rss`) and at its highest (`peak`)."""
        names = {"VmRSS": "rss", "VmHWM": "peak"}
        memory = {}
        with open(f"/proc/{self.process.pid}/status") as status:
            for line in status:
                key, _, value = line.partition(":")
                if key in names:
                    memory[names[key]] = int(value.split()[0]) / 1024
        return memory

    def cpu_ms(self):
        """CPU time the process has used, in user and kernel mode."""
        with open(f"/proc/{self.process.pid}/stat") as stat:
            fields = stat.read().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) * 1000 / CLOCK_TICKS


def connect():
    return http.client.HTTPConnection("127.0.0.1", PORT)


def get(connection, path):
    """GET `path`; returns (milliseconds, status, body)."""
    started = time.perf_counter()
    connection.request("GET", path)
    response = connection.getresponse()
    body = response.read()
    return (time.perf_counter() - started) * 1000, response.status, body


def measure_scenario(binary, config_path, scenario):
    with Service(binary, config_path) as service:
        idle = service.memory_mb()["rss"]
        connection = connect()
        first_ms, status, body = get(connection, scenario["path"])
        result = dict(scenario)
        result.update(
            status=status,
            startup_ms=round(service.startup_ms, 2),
            idle_mb=round(idle, 1),
            first_ms=round(first_ms, 2),
            bytes=len(body),
            sha256=hashlib.sha256(body).hexdigest(),
        )
        if status == 200:
            # At least three more, and up to ten while they are quick.
            latencies = []
            cpu_before = service.cpu_ms()
            started = time.perf_counter()
            while len(latencies) < 3 or (
                len(latencies) < 10 and time.perf_counter() - started < 1.5
            ):
                latency, status, _ = get(connection, scenario["path"])
                if status != 200:
                    raise RuntimeError(f"{scenario['id']}: status {status} on a repeat")
                latencies.append(latency)
            result.update(
                min_ms=round(min(latencies), 2),
                median_ms=round(statistics.median(latencies), 2),
                cpu_ms=round((service.cpu_ms() - cpu_before) / len(latencies), 1),
            )
        result["peak_mb"] = round(service.memory_mb()["peak"], 1)
        return result


def run_load(service, next_path):
    """Request from CONCURRENCY keep-alive connections until `next_path`
    returns None; returns the latencies and the wall and CPU time taken."""
    latencies = []
    failures = []
    lock = threading.Lock()

    def worker():
        connection = connect()
        while True:
            with lock:
                path = next_path()
            if path is None:
                return
            latency, status, _ = get(connection, path)
            with lock:
                latencies.append(latency)
                if status != 200:
                    failures.append((status, path))

    cpu_before = service.cpu_ms()
    started = time.perf_counter()
    workers = [threading.Thread(target=worker) for _ in range(CONCURRENCY)]
    for thread in workers:
        thread.start()
    for thread in workers:
        thread.join()
    wall = time.perf_counter() - started
    if failures:
        raise RuntimeError(f"requests failed under load: {failures[:3]}")
    latencies.sort()
    return {
        "requests": len(latencies),
        "requests_per_second": round(len(latencies) / wall, 2),
        "median_ms": round(statistics.median(latencies), 2),
        "p99_ms": round(latencies[int(len(latencies) * 0.99)], 2),
        "cpu_seconds": round((service.cpu_ms() - cpu_before) / 1000, 2),
    }


def measure_throughput(binary, config_path):
    path = "/images/fs/photo-0.4mp.jpg/w320.webp"
    with Service(binary, config_path) as service:
        connection = connect()
        for _ in range(3):
            get(connection, path)
        deadline = time.perf_counter() + THROUGHPUT_SECONDS
        result = run_load(service, lambda: path if time.perf_counter() < deadline else None)
        result.update(path=path, concurrency=CONCURRENCY, peak_mb=round(service.memory_mb()["peak"], 1))
        return result


def measure_burst(binary, config_path):
    paths = [s["path"] for s in scenarios() if s["group"] == "grid" and s["width"] == 1280]
    random.Random(7).shuffle(paths)
    queue = collections.deque(paths * BURST_ROUNDS)
    with Service(binary, config_path) as service:
        result = run_load(service, lambda: queue.popleft() if queue else None)
        memory = service.memory_mb()
        time.sleep(SETTLE_SECONDS)
        result.update(
            concurrency=CONCURRENCY,
            peak_mb=round(memory["peak"], 1),
            after_mb=round(memory["rss"], 1),
            settled_mb=round(service.memory_mb()["rss"], 1),
        )
        return result


def corpus_digest(corpus):
    """One hash over the names and contents of every source image."""
    digest = hashlib.sha256()
    for name in sorted(os.listdir(corpus)):
        with open(os.path.join(corpus, name), "rb") as source:
            digest.update(f"{name} {hashlib.sha256(source.read()).hexdigest()}\n".encode())
    return digest.hexdigest()


def cmd_run(args):
    selected = [s for s in scenarios() if args.only in s["id"]]
    fixture_server = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(HTTP_FIXTURE_PORT), "-d", args.corpus],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        with tempfile.NamedTemporaryFile("w", suffix=".toml") as config:
            config.write(
                CONFIG.format(
                    port=PORT,
                    http_port=HTTP_FIXTURE_PORT,
                    corpus=args.corpus,
                    concurrency=CONCURRENCY,
                )
            )
            config.flush()
            matrix = []
            for number, scenario in enumerate(selected, 1):
                print(f"[{number}/{len(selected)}] {scenario['id']}", file=sys.stderr, flush=True)
                matrix.append(measure_scenario(args.binary, config.name, scenario))
            results = {
                "label": args.label,
                "machine": {
                    "architecture": platform.machine(),
                    "cpus": os.cpu_count(),
                    "kernel": platform.release(),
                },
                "corpus": corpus_digest(args.corpus),
                "matrix": matrix,
            }
            # The load measurements cover the whole corpus, so a run
            # narrowed with --only leaves them out.
            if not args.only:
                print("throughput", file=sys.stderr, flush=True)
                results["throughput"] = measure_throughput(args.binary, config.name)
                print("burst", file=sys.stderr, flush=True)
                results["burst"] = measure_burst(args.binary, config.name)
    finally:
        fixture_server.kill()
        fixture_server.wait()
    json.dump(results, sys.stdout, indent=1)
    print()


# ---------------------------------------------------------------- report

# (heading, scenario filter, grouping key): the ways the matrix is cut.
VIEWS = [
    ("by content, 9.4 MP sources", lambda s: s["group"] == "grid", lambda s: s["content"]),
    ("by source format, 9.4 MP sources", lambda s: s["group"] == "grid", lambda s: s["source_format"]),
    ("by output format", lambda s: s["group"] != "http", lambda s: s["format"]),
    ("by output width, 9.4 MP sources", lambda s: s["group"] == "grid", lambda s: f"{s['width']:>5}"),
    (
        "by source size, photo content",
        lambda s: s["content"] == "photo" and s["width"] != 3840 and s["group"] != "http",
        lambda s: f"{float(s['size'][:-2]):4.1f} MP",
    ),
    ("by source encoding, to 1280 wide", lambda s: s["group"] == "variant", lambda s: s["source"]),
    ("fetched over HTTP, to 1280 wide WebP", lambda s: s["group"] == "http", lambda s: s["source"]),
]


def load(path):
    with open(path) as results:
        return json.load(results)


def grouped(matrix, keep, key):
    groups = collections.defaultdict(list)
    for scenario in matrix:
        if scenario["status"] == 200 and keep(scenario):
            groups[key(scenario)].append(scenario)
    return sorted(groups.items())


def report_one(results):
    matrix = results["matrix"]
    machine = results["machine"]
    print(
        f"{results['label']}: {len(matrix)} scenarios on {machine['cpus']} "
        f"{machine['architecture']} CPUs, corpus {results['corpus'][:12]}"
    )
    refused = [s for s in matrix if s["status"] != 200]
    for scenario in refused:
        print(f"  status {scenario['status']}: {scenario['id']}")
    served = [s for s in matrix if s["status"] == 200]
    print(
        f"startup {statistics.median(s['startup_ms'] for s in matrix):.1f} ms, "
        f"idle {statistics.median(s['idle_mb'] for s in matrix):.0f} MB, "
        f"first request {statistics.median(s['first_ms'] - s['min_ms'] for s in served):+.1f} ms "
        f"on the fastest repeat (medians)"
    )
    for heading, keep, key in VIEWS:
        groups = grouped(matrix, keep, key)
        if not groups:
            continue
        print(f"\n{heading} (medians)")
        print(f"  {'':22} {'n':>3} {'ms':>8} {'cpu ms':>8} {'peak MB':>8} {'out KB':>8}")
        for name, group in groups:
            print(
                f"  {name:22} {len(group):3d} "
                f"{statistics.median(s['min_ms'] for s in group):8.1f} "
                f"{statistics.median(s['cpu_ms'] for s in group):8.1f} "
                f"{statistics.median(s['peak_mb'] for s in group):8.0f} "
                f"{statistics.median(s['bytes'] for s in group) / 1024:8.0f}"
            )
    for heading, field, unit in [("slowest", "min_ms", "ms"), ("most memory", "peak_mb", "MB")]:
        print(f"\n{heading}")
        for scenario in sorted(served, key=lambda s: -s[field])[:8]:
            print(f"  {scenario['id']:36} {scenario[field]:8.0f} {unit}")
    report_load(results, None)


def ratio(group, baseline, field):
    """Geometric mean of new over old for `field` across `group`."""
    ratios = [
        s[field] / baseline[s["id"]][field]
        for s in group
        if s[field] > 0 and baseline[s["id"]][field] > 0
    ]
    return statistics.geometric_mean(ratios) if ratios else float("nan")


def report_two(old, new):
    if old["corpus"] != new["corpus"]:
        sys.exit(
            f"{old['label']} and {new['label']} were measured over different corpora "
            f"({old['corpus'][:12]} and {new['corpus'][:12]}): their numbers do not compare"
        )
    baseline = {s["id"]: s for s in old["matrix"]}
    matrix = [s for s in new["matrix"] if s["id"] in baseline]
    print(f"{new['label']} against {old['label']}: {len(matrix)} scenarios in both")
    for scenario in matrix:
        before = baseline[scenario["id"]]["status"]
        if scenario["status"] != before:
            print(f"  status {before} -> {scenario['status']}: {scenario['id']}")
    both = [s for s in matrix if s["status"] == 200 and baseline[s["id"]]["status"] == 200]
    identical = sum(s["sha256"] == baseline[s["id"]]["sha256"] for s in both)
    print(f"responses byte-identical: {identical} of {len(both)}")
    if identical < len(both):
        changed = collections.Counter(
            s["format"] for s in both if s["sha256"] != baseline[s["id"]]["sha256"]
        )
        print("  changed, by output format: " + ", ".join(f"{k} {v}" for k, v in sorted(changed.items())))
    fields = [("ms", "min_ms"), ("cpu", "cpu_ms"), ("first", "first_ms"), ("peak", "peak_mb"), ("bytes", "bytes")]
    print("\nnew / old, geometric means (below 1.00 is less)")
    header = f"  {'':22} {'n':>3} " + " ".join(f"{name:>6}" for name, _ in fields)
    print(header)
    print(f"  {'all':22} {len(both):3d} " + " ".join(f"{ratio(both, baseline, f):6.2f}" for _, f in fields))
    for heading, keep, key in VIEWS:
        groups = grouped(both, keep, key)
        if not groups:
            continue
        print(f"\n{heading}")
        print(header)
        for name, group in groups:
            print(
                f"  {name:22} {len(group):3d} "
                + " ".join(f"{ratio(group, baseline, f):6.2f}" for _, f in fields)
            )
    for heading, field, unit in [("time", "min_ms", "ms"), ("peak memory", "peak_mb", "MB")]:
        print(f"\nlargest increases in {heading}")
        worst = sorted(both, key=lambda s: -s[field] / baseline[s["id"]][field])[:8]
        for scenario in worst:
            before = baseline[scenario["id"]][field]
            print(
                f"  {scenario['id']:36} {before:8.1f} -> {scenario[field]:8.1f} {unit} "
                f"x{scenario[field] / before:.2f}"
            )
    report_load(new, old)


def report_load(results, old):
    """The throughput and burst measurements, beside `old`'s if given."""
    rows = [
        ("throughput", "requests_per_second", "requests/s"),
        ("throughput", "p99_ms", "ms p99"),
        ("burst", "requests_per_second", "requests/s"),
        ("burst", "p99_ms", "ms p99"),
        ("burst", "cpu_seconds", "CPU s"),
        ("burst", "peak_mb", "MB peak"),
        ("burst", "after_mb", "MB right after"),
        ("burst", "settled_mb", f"MB {SETTLE_SECONDS} s later"),
    ]
    if "burst" not in results or (old is not None and "burst" not in old):
        return
    print(f"\nunder load, {results['burst']['concurrency']} connections")
    for section, field, unit in rows:
        value = results[section][field]
        before = f"{old[section][field]:9.1f} -> " if old is not None else ""
        print(f"  {section:11} {before}{value:9.1f} {unit}")


def cmd_report(args):
    if args.new is None:
        report_one(load(args.old))
    else:
        report_two(load(args.old), load(args.new))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="measure; JSON on stdout")
    run.add_argument("--binary", default="/usr/local/bin/pixtega")
    run.add_argument("--corpus", default="/corpus")
    run.add_argument("--label", default="unnamed")
    run.add_argument("--only", default="", help="only scenarios whose id contains this text")
    run.set_defaults(func=cmd_run)
    report = commands.add_parser("report", help="print one run, or a second against the first")
    report.add_argument("old", metavar="A.json")
    report.add_argument("new", metavar="B.json", nargs="?")
    report.set_defaults(func=cmd_report)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
