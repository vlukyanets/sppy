#!/bin/sh
# Builds NAME-256.png and NAME.ico from NAME.svg for each icon. Needs rsvg-convert
# (librsvg) and python3. The ICO holds PNG images (Windows Vista and later), which
# keeps it small.
set -eu
cd "$(dirname "$0")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for name in sppy sppy-server; do
    rsvg-convert -w 256 -h 256 "$name.svg" -o "$name-256.png"
    for n in 16 24 32 48 64 128 256; do
        rsvg-convert -w $n -h $n "$name.svg" -o "$tmp/$name-$n.png"
    done
    python3 - "$tmp" "$name" <<'PY'
import struct, sys
tmp, name = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 128, 256]
pngs = [open(f"{tmp}/{name}-{n}.png", "rb").read() for n in sizes]
head, body, offset = struct.pack("<HHH", 0, 1, len(sizes)), b"", 6 + 16 * len(sizes)
for n, png in zip(sizes, pngs):
    head += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(png), offset)
    offset, body = offset + len(png), body + png
open(f"{name}.ico", "wb").write(head + body)
PY
done
