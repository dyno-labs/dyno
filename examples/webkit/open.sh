#!/usr/bin/env bash
# Opens one URL in WebKitGTK on a virtual display the size of the reference phone's screen, at
# its pixel ratio, with a fresh private profile, until it is stopped (open.py beside it opens the
# window). It is the program for a run in WebKit, Safari's engine, on Linux:
#
#   dyno check <build> --driver serve --serve-host 127.0.0.1 --open-with examples/webkit/open.sh
#
# dyno starts it with the harness page's URL, reads the memory of every process under it, and
# stops its whole process group when the run ends, the virtual display with it. It needs Xvfb
# (Arch: xorg-server-xvfb), WebKitGTK (webkit2gtk-4.1) and its Python bindings (python-gobject).
#
#   WIDTH, HEIGHT    the viewport in CSS px (default 430 x 932, the iPhone 14 Pro Max)
#   SCALE            the device pixel ratio, a whole number (default 3)
#   MEMORY_LIMIT_MB  a memory limit for WebKit's web process (see open.py)
set -euo pipefail

url="${1:?usage: examples/webkit/open.sh <url>}"
width="${WIDTH:-430}"
height="${HEIGHT:-932}"
scale="${SCALE:-3}"

if ! command -v Xvfb >/dev/null; then
	echo "examples/webkit/open.sh needs Xvfb (Arch: pacman -S xorg-server-xvfb)" >&2
	exit 1
fi

# Xvfb picks a free display and writes its number to the file.
display_file="$(mktemp)"
trap 'rm -f "$display_file"' EXIT
Xvfb -displayfd 3 -screen 0 "$((width * scale))x$((height * scale))x24" -nolisten tcp \
	3>"$display_file" 2>/dev/null &
for _ in $(seq 100); do
	[[ -s "$display_file" ]] && break
	sleep 0.05
done
if [[ ! -s "$display_file" ]]; then
	echo "examples/webkit/open.sh: Xvfb did not start" >&2
	exit 1
fi
display=":$(tr -d '[:space:]' <"$display_file")"
rm -f "$display_file"
trap - EXIT

# GTK scales its windows by GDK_SCALE on X11, so the view is WIDTH x HEIGHT CSS px at SCALE
# device pixels each. Xvfb stays a child of this process when Python replaces it.
export DISPLAY="$display" GDK_BACKEND=x11 GDK_SCALE="$scale" GDK_DPI_SCALE=1
export WIDTH="$width" HEIGHT="$height"
exec python3 -I "$(dirname "$0")/open.py" "$url"
