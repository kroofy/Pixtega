#!/usr/bin/env bash
# Benchmark a pixtega container image: what it costs in time, CPU, and
# memory to derive every output format, at several widths, from sources
# that differ in content, format, size, and encoding.
#
# Usage: scripts/bench/bench.sh LABEL [IMAGE] [bench.py run options...]
#   LABEL  names the run: results go to bench/results/LABEL.json
#   IMAGE  defaults to pixtega-acceptance:local, which
#          scripts/container-acceptance.sh builds
#
# To see what a change did, benchmark an image from before it and one from
# after it, then compare:
#
#   scripts/bench/bench.sh before pixtega:before
#   scripts/bench/bench.sh after pixtega:after
#   scripts/bench/bench.py report bench/results/before.json bench/results/after.json
#
# The image is wrapped in a benchmark image that adds the corpus and
# bench.py (see the Dockerfile here), so the service runs with exactly the
# libraries it ships with, over the same source images every time. The
# first build downloads the photographs and generates the corpus, which
# takes a minute; later builds reuse it. A full run takes a few minutes;
# `--only TEXT` narrows it to the scenarios whose id contains TEXT.
#
# Requires Docker and python3.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ $# -lt 1 ]; then
    sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0" >&2
    exit 2
fi
LABEL=$1
shift
IMAGE=pixtega-acceptance:local
if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
    IMAGE=$1
    shift
fi

BENCH_IMAGE=pixtega-bench:local
RESULTS=bench/results/${LABEL}.json

echo "==> building ${BENCH_IMAGE} from ${IMAGE}" >&2
docker build -q -f scripts/bench/Dockerfile --build-arg IMAGE="$IMAGE" \
    -t "$BENCH_IMAGE" scripts/bench >/dev/null

mkdir -p bench/results
docker run --rm "$BENCH_IMAGE" run --label "$LABEL" "$@" >"${RESULTS}.partial"
mv "${RESULTS}.partial" "$RESULTS"
echo "==> wrote ${RESULTS}" >&2
python3 scripts/bench/bench.py report "$RESULTS"
