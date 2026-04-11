#!/usr/bin/env bash
# Regenerate all PNG icons from the scalable SVG source.
# Requires ImageMagick (magick / convert).
set -euo pipefail

SVG="$(dirname "$0")/icons/hicolor/scalable/apps/s3-explorer.svg"

for size in 16 32 48 64 128 256 512; do
    out="$(dirname "$0")/icons/hicolor/${size}x${size}/apps/s3-explorer.png"
    mkdir -p "$(dirname "$out")"
    magick -background none "$SVG" -resize "${size}x${size}" "$out"
    echo "Generated ${size}x${size}"
done
