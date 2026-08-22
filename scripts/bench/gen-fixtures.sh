#!/usr/bin/env bash
# Generate the benchmark image corpus under bench/fixtures/ using the vips CLI.
# The corpus is deterministic (fixed noise seed is not available in the vips
# CLI, but sizes and structure are fixed; absolute byte sizes may vary a little
# between libvips versions).
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=bench/fixtures
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"

SRC=fixtures/photos/example.jpg

# small: 200px thumbnail of the bundled photo (~7 KB)
vips thumbnail "$SRC" "$OUT/small.jpg[Q=85,strip]" 200

# medium webp source: 2000px upscale saved as WebP
vips resize "$SRC" "$TMP/up2k.v" 1.5625
vips webpsave "$TMP/up2k.v" "$OUT/medium.webp" --Q 90 --strip

# large smooth photo: 8000x4000 (32 MP) upscale, JPEG q90
vips resize "$SRC" "$TMP/big.v" 6.25
vips jpegsave "$TMP/big.v" "$OUT/large-photo.jpg" --Q 90 --strip

# complex: 4000x3000 (12 MP) 3-band gaussian noise, JPEG q90.
# Noise is the worst case for both the JPEG decoder and every encoder.
for b in 1 2 3; do
  vips gaussnoise "$TMP/n$b.v" 4000 3000 --sigma 60 --mean 128
done
vips bandjoin "$TMP/n1.v $TMP/n2.v $TMP/n3.v" "$TMP/noise.v"
vips cast "$TMP/noise.v" "$TMP/noise_u.v" uchar
vips jpegsave "$TMP/noise_u.v" "$OUT/complex.jpg" --Q 90 --strip

# large PNG with alpha: 3000x2000 noise RGB + horizontal-gradient alpha.
# PNG has no shrink-on-load, so this exercises a full-resolution decode,
# and the alpha band exercises the JPEG flatten path.
for b in 1 2 3; do
  vips gaussnoise "$TMP/p$b.v" 3000 2000 --sigma 60 --mean 128
done
vips xyz "$TMP/xy.v" 3000 2000
vips extract_band "$TMP/xy.v" "$TMP/x.v" 0
vips linear "$TMP/x.v" "$TMP/alpha.v" 0.085 0
vips bandjoin "$TMP/p1.v $TMP/p2.v $TMP/p3.v $TMP/alpha.v" "$TMP/rgba.v"
vips cast "$TMP/rgba.v" "$TMP/rgba_u.v" uchar
vips pngsave "$TMP/rgba_u.v" "$OUT/large-alpha.png" --compression 6 --strip

# icc: 2000px photo with an embedded sRGB ICC profile, to verify the
# profile-carrying path still gets a real ICC transform.
vips jpegsave "$TMP/up2k.v" "$OUT/icc.jpg" --Q 90 --profile srgb

ls -la "$OUT"
for f in "$OUT"/*; do vipsheader "$f"; done
