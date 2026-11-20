#!/usr/bin/env bash
# The benchmark corpus recipe. scripts/bench/Dockerfile runs it with the
# vips CLI of a pinned Ubuntu release, so every run measures the same
# source images; it is not meant to be run by hand.
#
# Usage: gen-corpus.sh KODAK_DIR OUT_DIR
#   KODAK_DIR  the 24 Kodak test photographs (kodim01.png ... kodim24.png)
#
# Four kinds of content, each 4608x2048 (9.4 MP) in JPEG, PNG, WebP, and
# AVIF, because what an image costs depends on what is in it:
#
#   photo    the Kodak photographs side by side at their native resolution
#   smooth   one of them enlarged six times: no fine detail at all
#   graphic  text and flat colour blocks, like a screenshot
#   noise    gaussian noise, the worst case for every codec
#
# Then the photo content at 0.4, 1.6, and 38 MP in all four formats, and
# the encodings a source can arrive in (var-*): progressive, 4:2:0,
# grayscale, CMYK, and EXIF-rotated JPEG; interlaced, 16-bit, palette, and
# transparent PNG; lossless and transparent WebP; transparent AVIF.
set -euo pipefail

KODAK=$1 OUT=$2
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$OUT"

W=4608 H=2048

# save_all IMAGE NAME: the four source formats of one image.
save_all() {
    vips jpegsave "$1" "$OUT/$2.jpg" --Q 90 --strip
    vips pngsave "$1" "$OUT/$2.png" --compression 6 --strip
    vips webpsave "$1" "$OUT/$2.webp" --Q 90 --strip
    vips heifsave "$1" "$OUT/$2.avif" --Q 60 --compression av1 --strip
}

# photo: six across, four down, the portrait photographs turned on their side
tiles=()
for f in "$KODAK"/kodim*.png; do
    if [ "$(vipsheader -f width "$f")" = 512 ]; then
        vips rot "$f" "$T/$(basename "$f" .png).v" d90
        tiles+=("$T/$(basename "$f" .png).v")
    else
        tiles+=("$f")
    fi
done
vips arrayjoin "${tiles[*]}" "$T/photo.v" --across 6

vips resize "$KODAK/kodim23.png" "$T/enlarged.v" 6
vips crop "$T/enlarged.v" "$T/smooth.v" 0 512 $W $H

for band in 1 2 3; do
    vips gaussnoise "$T/noise$band.v" $W $H --sigma 60 --mean 128 --seed "$band"
done
vips bandjoin "$T/noise1.v $T/noise2.v $T/noise3.v" "$T/noise-float.v"
vips cast "$T/noise-float.v" "$T/noise.v" uchar

# graphic: tinted text on white, with four flat rectangles laid over it
TEXT="Pixtega derives images on demand. The quick brown fox jumps over the lazy dog 0123456789. Sphinx of black quartz, judge my vow. Pack my box with five dozen liquor jugs. How vexingly quick daft zebras jump."
vips text "$T/text.v" "$TEXT $TEXT $TEXT" --width 1500 --dpi 130 --font "DejaVu Sans 11"
vips replicate "$T/text.v" "$T/text-tiled.v" 4 24
vips crop "$T/text-tiled.v" "$T/text-page.v" 0 0 $W $H
vips linear "$T/text-page.v" "$T/ink.v" "1 0.9 0.75" "0 0 0" --uchar
vips invert "$T/ink.v" "$T/page.v"
vips copy "$T/page.v" "$T/graphic0.v" --interpretation srgb
n=0
for block in "220 40 40 300 200 900 500" "30 120 220 1700 900 1200 700" \
             "40 170 90 3300 150 1000 1300" "250 200 30 600 1300 700 600"; do
    read -r red green blue left top width height <<<"$block"
    vips black "$T/black.v" "$width" "$height" --bands 3
    vips linear "$T/black.v" "$T/block.v" 1 "$red $green $blue" --uchar
    vips insert "$T/graphic$n.v" "$T/block.v" "$T/graphic$((n + 1)).v" "$left" "$top"
    n=$((n + 1))
done
vips copy "$T/graphic$n.v" "$T/graphic.v"

for content in photo smooth graphic noise; do
    save_all "$T/$content.v" "$content-9.4mp"
done

# Source sizes: one photograph, four, and the whole set four times over.
vips arrayjoin "$KODAK/kodim23.png $KODAK/kodim05.png $KODAK/kodim08.png $KODAK/kodim13.png" \
    "$T/photo-1.6mp.v" --across 2
vips replicate "$T/photo.v" "$T/photo-38mp.v" 2 2
save_all "$KODAK/kodim23.png" photo-0.4mp
save_all "$T/photo-1.6mp.v" photo-1.6mp
save_all "$T/photo-38mp.v" photo-38mp

# Encodings of the 9.4 MP photo content.
vips jpegsave "$T/photo.v" "$OUT/var-progressive.jpg" --Q 90 --strip --interlace
vips jpegsave "$T/photo.v" "$OUT/var-420.jpg" --Q 90 --strip --subsample-mode on
vips colourspace "$T/photo.v" "$T/gray.v" b-w
vips jpegsave "$T/gray.v" "$OUT/var-gray.jpg" --Q 90 --strip
vips colourspace "$T/photo.v" "$T/cmyk.v" cmyk
vips jpegsave "$T/cmyk.v" "$OUT/var-cmyk.jpg" --Q 90

# EXIF orientation 6 (rotate 90 degrees to display), as phones write for
# portrait shots: an APP1 segment spliced in right after the two byte SOI
# marker, where Exif puts it, since the vips CLI cannot set the tag.
head -c 2 "$OUT/photo-9.4mp.jpg" >"$OUT/var-rotated.jpg"
printf '\xff\xe1\x00\x22Exif\x00\x00II*\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00' \
    >>"$OUT/var-rotated.jpg"
tail -c +3 "$OUT/photo-9.4mp.jpg" >>"$OUT/var-rotated.jpg"
[ "$(vipsheader -f orientation "$OUT/var-rotated.jpg")" = 6 ]

vips pngsave "$T/photo.v" "$OUT/var-interlaced.png" --compression 6 --strip --interlace
vips pngsave "$T/graphic.v" "$OUT/var-palette.png" --palette --strip
vips linear "$T/photo.v" "$T/photo16-float.v" 257 0
vips cast "$T/photo16-float.v" "$T/photo16-plain.v" ushort
vips copy "$T/photo16-plain.v" "$T/photo16.v" --interpretation rgb16
vips pngsave "$T/photo16.v" "$OUT/var-16bit.png" --compression 6 --strip

# Transparency: opaque on the left, fading out to the right.
vips xyz "$T/xy.v" $W $H
vips extract_band "$T/xy.v" "$T/x.v" 0
vips linear "$T/x.v" "$T/alpha.v" 0.0553 0 --uchar
vips bandjoin "$T/photo.v $T/alpha.v" "$T/rgba.v"
vips pngsave "$T/rgba.v" "$OUT/var-alpha.png" --compression 6 --strip
vips webpsave "$T/rgba.v" "$OUT/var-alpha.webp" --Q 90 --strip
vips heifsave "$T/rgba.v" "$OUT/var-alpha.avif" --Q 60 --compression av1 --strip
vips webpsave "$T/photo.v" "$OUT/var-lossless.webp" --lossless --strip
