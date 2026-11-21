# Icons

`sppy.svg` is the source. The others are made from it; after changing it, run:

```sh
rsvg-convert -w 256 -h 256 assets/sppy.svg -o assets/sppy-256.png   # window icon
for n in 16 24 32 48 64 128 256; do rsvg-convert -w $n -h $n assets/sppy.svg -o /tmp/sppy-$n.png; done
python3 - <<'PY'                                                      # sppy.exe icon
import struct
sizes = [16, 24, 32, 48, 64, 128, 256]
pngs = [open(f"/tmp/sppy-{n}.png", "rb").read() for n in sizes]
head, body, offset = struct.pack("<HHH", 0, 1, len(sizes)), b"", 6 + 16 * len(sizes)
for n, png in zip(sizes, pngs):
    head += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(png), offset)
    offset, body = offset + len(png), body + png
open("assets/sppy.ico", "wb").write(head + body)
PY
```

The ICO holds PNG images (Windows Vista and later), which keeps it at about 35 KB.
