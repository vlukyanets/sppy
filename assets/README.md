# Icons

`sppy.svg` (the app) and `sppy-server.svg` (the headless server) are the sources. The
PNG and ICO files are made from them; after changing an SVG, run `assets/make-icons.sh`
(needs `rsvg-convert` and `python3`).

- `sppy-256.png`: the app's window icon (built into `sppy`); `sppy-server-256.png` is
  for places that need a raster image of the server's icon.
- `*.ico`: the icons of `sppy.exe` and `sppy-server.exe` on Windows.
