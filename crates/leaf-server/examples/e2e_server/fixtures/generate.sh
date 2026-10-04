#!/bin/sh
# Regenerates the media the e2e server seeds its in-memory store with.
#
# The files are committed; run this only to change them. Needs ffmpeg (with
# libx264 and libvpx-vp9) and cwebp. The bytes differ between ffmpeg builds,
# which is fine: nothing depends on them beyond "small, valid, this type".
#
# Thumbnails are WebP at most 256 px on the long edge, like the ones
# leaf_core::media stores, because /api/media always serves a thumbnail as
# image/webp.
set -eu
cd "$(dirname "$0")"

ff() { ffmpeg -hide_banner -loglevel error -nostdin -y "$@"; }

# thumb <source image> <output.webp>
thumb() {
    ff -i "$1" -frames:v 1 -update 1 \
        -vf "scale='min(256,iw)':'min(256,ih)':force_original_aspect_ratio=decrease" \
        thumb-tmp.png
    cwebp -quiet -q 80 thumb-tmp.png -o "$2"
    rm -f thumb-tmp.png
}

# still <output> <size> <from colour> <to colour> <pixel format>: one
# diagonal gradient.
still() {
    w=${2%x*}
    h=${2#*x}
    ff -f lavfi -i "gradients=s=$2:c0=$3:c1=$4:x0=0:y0=0:x1=$w:y1=$h:n=2:d=1" \
        -frames:v 1 -update 1 -pix_fmt "$5" "$1"
    thumb "$1" "${1%.*}.thumb.webp"
}

# One still per shape the gallery has to lay out, in both image formats.
still landscape.png 640x480 0x2d6a4f 0xd8f3dc rgb24
still portrait.jpg 480x640 0x9d4edd 0xffc8dd yuvj420p
still square.png 512x512 0xe76f51 0xf4e285 rgb24
still wide.jpg 800x400 0x1d3557 0xa8dadc yuvj420p
still tall.png 360x720 0x6a040f 0xffba08 rgb24

# A few seconds of H.264 + AAC with the index up front (faststart), so a
# player can begin on the first bytes and seek with Range requests.
ff -f lavfi -i "testsrc2=s=480x270:r=24:d=4" -f lavfi -i "sine=f=440:d=4" \
    -c:v libx264 -profile:v baseline -pix_fmt yuv420p -crf 30 -g 24 \
    -c:a aac -b:a 48k -ac 1 -movflags +faststart -shortest clip.mp4
# The same clip as VP9 + Opus, for browsers built without H.264.
ff -f lavfi -i "testsrc2=s=480x270:r=24:d=4" -f lavfi -i "sine=f=660:d=4" \
    -c:v libvpx-vp9 -crf 48 -b:v 0 -g 24 -c:a libopus -b:a 32k -shortest clip.webm

# Poster frames, one second in, as leaf_core::media takes them.
for clip in clip.mp4 clip.webm; do
    ff -ss 1 -i "$clip" -frames:v 1 -update 1 poster-tmp.png
    thumb poster-tmp.png "$clip.thumb.webp"
    rm -f poster-tmp.png
done
