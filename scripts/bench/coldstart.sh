#!/usr/bin/env bash
# Cold start probe for a pixtega container image.
#
# Starts the service inside the image RUNS times and reports the median of:
#
#   listen ms   exec to the "listening" line: what a cold start waits for
#   first ms    the first request after that
#   read MB     bytes read from disk up to and including that request
#   idle MB     resident memory once listening
#   libs        shared libraries mapped once listening
#
# Twice: with the image's files evicted from the page cache before every
# start (a new host, or a Lambda execution environment, has nothing
# cached), and with the cache warm. `read MB` is the number to watch. It is
# deterministic where the timings depend on this machine's disk, and on
# Lambda, where image blocks are fetched on first touch, it is what the
# cold start mostly consists of.
#
# The configuration is passed inline through CONFIG, so the same probe runs
# against the service image and the Lambda image.
#
# Usage: scripts/bench/coldstart.sh [IMAGE] [RUNS]
#   IMAGE  defaults to pixtega-acceptance:local, which
#          scripts/container-acceptance.sh builds
#   RUNS   defaults to 10
set -euo pipefail
cd "$(dirname "$0")/../.."

IMAGE="${1:-pixtega-acceptance:local}"
RUNS="${2:-10}"
REQUEST_PATH="/images/fixtures/photos/example.6acca6c1.jpg/w640.webp"

# Prints one line per run: listen_us first_us read_kb idle_kb libs.
# Runs as root so the probe may read the service's /proc entries whatever
# user the image declares.
probe() {
    docker run --rm -i --user root -e CONFIG="$(cat config.example.toml)" \
        --entrypoint bash "$IMAGE" -s "$1" "$RUNS" "$REQUEST_PATH" <<'PROBE'
set -euo pipefail
cache=$1 runs=$2 path=$3

now_us() {
    local now=$EPOCHREALTIME
    echo "${now/[.,]/}"
}

# Reading zero bytes with dd's nocache flag asks the kernel to drop every
# cached page of that file.
evict_page_cache() {
    find /usr /etc /app -xdev -type f \
        -exec dd if={} iflag=nocache count=0 status=none \; 2>/dev/null || true
}

for _ in $(seq "$runs"); do
    if [ "$cache" = cold ]; then
        evict_page_cache
    fi

    started=$(now_us)
    coproc SERVICE { exec /usr/local/bin/pixtega 2>/dev/null; }
    pid=$SERVICE_PID
    read -r _ <&"${SERVICE[0]}"
    listening=$(now_us)
    idle_kb=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")
    libs=$(awk '$6 ~ /\.so/ {print $6}' "/proc/$pid/maps" | sort -u | wc -l)

    requested=$(now_us)
    exec 3<>/dev/tcp/127.0.0.1/8080
    printf 'GET %s HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n' "$path" >&3
    read -r _ status _ <&3
    cat <&3 >/dev/null
    exec 3<&-
    answered=$(now_us)
    read_kb=$(awk '/^read_bytes:/ {print int($2 / 1024)}' "/proc/$pid/io")

    kill -9 "$pid"
    wait "$pid" 2>/dev/null || true
    if [ "$status" != 200 ]; then
        echo "request failed with status $status" >&2
        exit 1
    fi
    echo "$((listening - started)) $((answered - requested)) $read_kb $idle_kb $libs"
done
PROBE
}

# Median of column $1 over the runs on stdin, divided by $2.
median() {
    cut -d' ' -f"$1" | sort -n | awk -v scale="$2" '
        { value[NR] = $1 }
        END { printf "%.1f", value[int((NR + 1) / 2)] / scale }'
}

report() {
    local label=$1 runs
    runs="$(probe "$2")"
    printf '%-12s %9s %8s %7s %7s %5s\n' "$label" \
        "$(median 1 1000 <<<"$runs")" \
        "$(median 2 1000 <<<"$runs")" \
        "$(median 3 1024 <<<"$runs")" \
        "$(median 4 1024 <<<"$runs")" \
        "$(median 5 1 <<<"$runs" | cut -d. -f1)"
}

echo "image: $IMAGE   runs: $RUNS (medians)"
printf '%-12s %9s %8s %7s %7s %5s\n' "" "listen ms" "first ms" "read MB" "idle MB" "libs"
report "cold cache" cold
report "warm cache" warm
