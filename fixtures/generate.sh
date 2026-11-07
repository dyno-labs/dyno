#!/usr/bin/env bash
# Rebuilds every fixture build folder under fixtures/<name>/build.
#
# It does not touch expected.toml. Those files are written by hand from the
# formulas in docs/design/static-scan.md. After a rebuild, run
# `git status fixtures` and, if any build file changed, check the
# expected.toml values of that fixture again (see fixtures/README.md).
#
# Needs: bash, ffmpeg (with libopus, libmp3lame, libsvtav1, libx264),
# oggenc (vorbis-tools), cwebp (webp), jpegtran (jpeg-turbo), basisu
# (basis_universal), rustc, npm and npx, zip, shasum.
#
# Usage: fixtures/generate.sh [fixture ...]   (no argument: all fixtures)
set -euo pipefail

FIXTURES="$(cd "$(dirname "$0")" && pwd)"
SRC="$FIXTURES/src"
ALL=(audio-formats howler-sprites images pixi8-inline pixi8-webgpu pixi7-aa pixi7-opts duplicates broken zip-input over-budget)
if [ "$#" -gt 0 ]; then WANT=("$@"); else WANT=("${ALL[@]}"); fi

for tool in ffmpeg oggenc cwebp jpegtran basisu rustc npm npx zip shasum; do
	command -v "$tool" >/dev/null || {
		echo "generate.sh: missing tool: $tool" >&2
		exit 1
	}
done

# All scratch work happens outside the repo, and is deleted on exit.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/dyno-fixtures.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
export TZ=UTC LC_ALL=C

# Make ffmpeg output byte-stable: no encoder version tags, no metadata.
FF=(ffmpeg -hide_banner -loglevel error -nostdin -y)
BX=(-fflags +bitexact -flags +bitexact -map_metadata -1 -map_chapters -1)

rustc -O --edition 2021 -o "$WORK/fixture_tool" "$SRC/tools/fixture_tool.rs"
TOOL="$WORK/fixture_tool"

want() {
	local n
	for n in "${WANT[@]}"; do [ "$n" = "$1" ] && return 0; done
	return 1
}

# Start a fixture: empty its build folder (only that folder) and cd into it.
begin() {
	local dir="$FIXTURES/$1/build"
	rm -rf "$dir"
	mkdir -p "$dir"
	cd "$dir"
	echo "fixture: $1"
}

# ---------- audio sources ----------
# Stereo test tone: 440 Hz left, 660 Hz right, so the channels differ.
stereo_tone() { # rate seconds out.wav
	"${FF[@]}" -f lavfi -i "aevalsrc=exprs=0.4*sin(2*PI*440*t)|0.4*sin(2*PI*660*t):s=$1:d=$2" -c:a pcm_s16le "${BX[@]}" "$3"
}
mono_tone() { # rate seconds out.wav
	"${FF[@]}" -f lavfi -i "aevalsrc=exprs=0.4*sin(2*PI*523.25*t):s=$1:d=$2" -c:a pcm_s16le "${BX[@]}" "$3"
}
# Gated tone: sound only where the gate expression is 1, exact silence elsewhere.
gated_stereo() { # rate seconds gate out.wav
	"${FF[@]}" -f lavfi -i "aevalsrc=exprs='0.4*sin(2*PI*440*t)*($3)|0.4*sin(2*PI*660*t)*($3)':s=$1:d=$2" -c:a pcm_s16le "${BX[@]}" "$4"
}
gated_mono() { # rate seconds gate out.wav
	"${FF[@]}" -f lavfi -i "aevalsrc=exprs='0.4*sin(2*PI*523.25*t)*($3)':s=$1:d=$2" -c:a pcm_s16le "${BX[@]}" "$4"
}
to_vorbis() { oggenc --quiet --quality 2 --serial "$3" -o "$2" "$1"; }                    # in out serial
to_opus() { "${FF[@]}" -i "$1" -c:a libopus -b:a "$3" "${BX[@]}" -f ogg "$2"; }           # in out bitrate
to_mp3_cbr() { "${FF[@]}" -i "$1" -c:a libmp3lame -b:a "$3" "${BX[@]}" "$2"; }            # in out bitrate
to_mp3_vbr() { "${FF[@]}" -i "$1" -c:a libmp3lame -q:a 4 "${BX[@]}" "$2"; }               # in out
to_adts() { "${FF[@]}" -i "$1" -c:a aac -b:a "$3" "${BX[@]}" -f adts "$2"; }              # in out bitrate
to_m4a() { "${FF[@]}" -i "$1" -c:a aac -b:a "$3" "${BX[@]}" "${@:4}" -f ipod "$2"; }      # in out bitrate [movflags...]
to_webm() { "${FF[@]}" -i "$1" -c:a libopus -b:a "$3" "${BX[@]}" "${@:4}" -f webm "$2"; } # in out bitrate [extra...]

# ---------- image sources ----------
png() { # out WxH pix_fmt source
	local sep="="
	case "$4" in *=*) sep=":" ;; esac
	"${FF[@]}" -f lavfi -i "$4${sep}s=$2,format=$3" -frames:v 1 -c:v png "${BX[@]}" "$1"
}
flat_png() { png "$1" "$2" "${3:-rgba}" "color=c=0x3366cc"; }
gradient_png() { png "$1" "$2" "${3:-rgba}" "testsrc2"; }
jpeg() { # out WxH
	"${FF[@]}" -f lavfi -i "testsrc2=s=$2" -frames:v 1 -c:v mjpeg -q:v 8 -pix_fmt yuvj420p "${BX[@]}" "$1"
}
avif() { # out WxH
	"${FF[@]}" -f lavfi -i "testsrc2=s=$2" -frames:v 1 -c:v libsvtav1 -pix_fmt yuv420p "${BX[@]}" -f avif "$1" 2>/dev/null
}
basis() { basisu -no_multithreading "$@" >/dev/null; }

# ---------- npm bundles ----------
bundle_setup() {
	[ -d "$WORK/npm/node_modules" ] && return 0
	mkdir -p "$WORK/npm"
	cp "$SRC/package.json" "$SRC/package-lock.json" "$WORK/npm/"
	# esbuild resolves packages from the entry file, so the apps sit next to node_modules.
	cp -R "$SRC/apps" "$WORK/npm/apps"
	(cd "$WORK/npm" && npm ci --no-audit --no-fund --loglevel=error >/dev/null)
}
bundle() { # app pixi-alias format out
	bundle_setup
	(cd "$WORK/npm" && npx --no-install esbuild "apps/$1.ts" --bundle --minify --target=es2020 \
		--format="$3" --alias:pixi.js="$2" --legal-comments=eof --log-level=warning --outfile="$4")
}
notices() { # fixture package...  -> fixtures/<fixture>/THIRD_PARTY_NOTICES.txt
	local fixture="$1"
	shift
	local out="$FIXTURES/$fixture/THIRD_PARTY_NOTICES.txt"
	{
		echo "Third-party code inside fixtures/$fixture/build (minified by esbuild)."
		echo "Each package below is bundled under its own licence, reproduced in full."
		local pkg dir files
		for pkg in "$@"; do
			dir="$WORK/npm/node_modules/$pkg"
			echo
			echo "================================================================"
			echo "$pkg $(node -p "require('$dir/package.json').version")"
			echo "================================================================"
			files="$(find "$dir" -maxdepth 1 -type f -iname 'licen[cs]e*' | sort)"
			if [ -n "$files" ]; then
				echo "$files" | while read -r f; do cat "$f"; done
			else
				mit_text "$dir"
			fi
		done
	} >"$out"
}
# For a package that ships no licence file: its package.json licence and
# author, then the standard MIT text (generate.sh stops on any other licence).
mit_text() {
	local license author
	license="$(node -p "require('$1/package.json').license || ''")"
	author="$(node -p "const a = require('$1/package.json').author; typeof a === 'string' ? a : (a && a.name) || ''")"
	[ "$license" = "MIT" ] || {
		echo "generate.sh: $1 has no licence file and is not MIT" >&2
		exit 1
	}
	echo "This package ships no licence file. Its package.json says: license MIT, author $author."
	echo
	echo "MIT License"
	echo
	echo "Copyright (c) $author"
	echo
	cat <<'MIT'
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
MIT
}
# Packages esbuild actually bundled for an app, from its metafile.
bundled_packages() { # app alias format
	bundle_setup
	(cd "$WORK/npm" && npx --no-install esbuild "apps/$1.ts" --bundle --minify --target=es2020 \
		--format="$3" --alias:pixi.js="$2" --log-level=error --metafile="$WORK/meta.json" --outfile="$WORK/meta.js")
	grep -o '"node_modules/[^"]*"' "$WORK/meta.json" | sed -E 's#"node_modules/((@[^/]+/)?[^/]+)/.*#\1#' | sort -u
}

# =====================================================================
if want audio-formats; then
	begin audio-formats
	mkdir -p assets/audio "$WORK/af"
	W="$WORK/af"
	stereo_tone 44100 1 assets/audio/tone-stereo-44k.wav
	mono_tone 22050 0.5 assets/audio/tone-mono-22k.wav
	stereo_tone 48000 1 "$W/s48.wav"
	mono_tone 48000 0.5 "$W/m48.wav"
	to_vorbis assets/audio/tone-stereo-44k.wav assets/audio/tone-stereo.ogg 1001
	to_vorbis assets/audio/tone-mono-22k.wav assets/audio/tone-mono.ogg 1002
	# Opus from a 44.1 kHz source. ffmpeg resamples to 48 kHz; the header then
	# gets the source rate back, as opusenc writes it. Granules stay 48 kHz.
	to_opus assets/audio/tone-stereo-44k.wav assets/audio/tone-stereo.opus 64k
	"$TOOL" opus-input-rate assets/audio/tone-stereo.opus 44100
	# Opus inside a .ogg name: the extension must not decide the codec.
	to_opus "$W/m48.wav" assets/audio/voice-mono-opus.ogg 32k
	to_mp3_cbr assets/audio/tone-stereo-44k.wav assets/audio/tone-stereo.mp3 128k
	to_mp3_vbr assets/audio/tone-mono-22k.wav assets/audio/tone-mono-22k.mp3
	"${FF[@]}" -i assets/audio/tone-stereo-44k.wav -c:a libmp3lame -b:a 128k -write_xing 0 "${BX[@]}" assets/audio/tone-noxing.mp3
	to_adts assets/audio/tone-stereo-44k.wav assets/audio/tone-stereo.aac 128k
	to_adts assets/audio/tone-mono-22k.wav assets/audio/tone-mono.aac 48k
	to_m4a assets/audio/tone-stereo-44k.wav assets/audio/tone-stereo.m4a 128k -movflags +faststart
	# moov after mdat (no faststart): the reader must find moov at the end.
	to_m4a assets/audio/tone-mono-22k.wav assets/audio/tone-mono.m4a 48k
	to_webm "$W/s48.wav" assets/audio/tone-stereo.webm 64k
	to_webm "$W/m48.wav" assets/audio/tone-mono.webm 32k
	# Live mode: no Duration element, like a MediaRecorder file.
	to_webm "$W/s48.wav" assets/audio/tone-live.webm 64k -live 1
fi

# =====================================================================
if want howler-sprites; then
	begin howler-sprites
	mkdir -p assets/audio "$WORK/hs"
	W="$WORK/hs"
	# sfx: 6 s stereo. Sound at [0, 0.5), [1.5, 2.25), [3, 4). Silence elsewhere.
	gated_stereo 44100 6 "lt(t,0.5)+gte(t,1.5)*lt(t,2.25)+gte(t,3)*lt(t,4)" "$W/sfx.wav"
	to_webm "$W/sfx.wav" assets/audio/sfx.webm 64k
	to_mp3_cbr "$W/sfx.wav" assets/audio/sfx.mp3 96k
	# music: 8 s stereo. Sound at [0, 4) and [5, 7).
	gated_stereo 44100 8 "lt(t,4)+gte(t,5)*lt(t,7)" "$W/music.wav"
	to_m4a "$W/music.wav" assets/audio/music.m4a 96k -movflags +faststart
	# ui: 2 s mono 22.05 kHz. Sound at [0, 0.25) and [1.25, 1.75).
	gated_mono 22050 2 "lt(t,0.25)+gte(t,1.25)*lt(t,1.75)" "$W/ui.wav"
	to_vorbis "$W/ui.wav" assets/audio/ui.ogg 1003
	to_mp3_vbr "$W/ui.wav" assets/audio/ui.mp3
	cat >assets/audio/music.json <<'EOF'
{
  "src": ["assets/audio/music.m4a"],
  "sprite": {
    "base": [0, 4000, true],
    "bonus": [5000, 2000, true]
  }
}
EOF
	cat >assets/audio/ui.json <<'EOF'
{
  "resources": ["ui.ogg", "ui.mp3"],
  "spritemap": {
    "tap": { "start": 0, "end": 0.25, "loop": false },
    "error": { "start": 1.25, "end": 1.75, "loop": false }
  }
}
EOF
	bundle howler-sprites pixi8 iife "$FIXTURES/howler-sprites/build/main.js"
	cp "$SRC/html/howler-sprites.html" index.html
	mapfile -t packages < <(bundled_packages howler-sprites pixi8 iife)
	notices howler-sprites "${packages[@]}"
fi

# =====================================================================
if want images; then
	begin images
	mkdir -p assets/{png,jpg,webp,avif,ktx2,basis,spine,svg,gif} "$WORK/im"
	W="$WORK/im"
	gradient_png assets/png/rgba-300x200.png 300x200 rgba
	gradient_png assets/png/rgb-256x256.png 256x256 rgb24
	gradient_png assets/png/palette-100x50.png 100x50 pal8
	gradient_png assets/png/gray16-64x64.png 64x64 gray16be
	flat_png assets/png/flat-2048x2048.png 2048x2048 rgba
	flat_png assets/png/bg-2080x1560.png 2080x1560 rgb24
	jpeg assets/jpg/baseline-640x480.jpg 640x480
	jpeg "$W/base-1024x768.jpg" 1024x768
	jpegtran -progressive -copy none -outfile assets/jpg/progressive-1024x768.jpg "$W/base-1024x768.jpg"
	gradient_png "$W/lossy.png" 512x256 rgb24
	cwebp -quiet -q 75 "$W/lossy.png" -o assets/webp/lossy-512x256.webp
	gradient_png "$W/lossless.png" 200x100 rgb24
	cwebp -quiet -lossless "$W/lossless.png" -o assets/webp/lossless-200x100.webp
	"${FF[@]}" -f lavfi -i "testsrc2=s=300x300,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lt(X,150),255,64)'" \
		-frames:v 1 -c:v png "${BX[@]}" "$W/alpha.png"
	cwebp -quiet -q 75 -alpha_q 80 "$W/alpha.png" -o assets/webp/alpha-300x300.webp
	avif assets/avif/photo-640x360.avif 640x360
	"$TOOL" ktx2-rgba8 assets/ktx2/rgba8-mips-64x64.ktx2 64 64 1
	"$TOOL" ktx2-astc assets/ktx2/astc-256x128.ktx2 256 128
	flat_png "$W/flat-1024.png" 1024x1024 rgba
	basis -ktx2 -mipmap -file "$W/flat-1024.png" -output_file assets/ktx2/etc1s-mips-1024x1024.ktx2
	flat_png "$W/flat-512x256.png" 512x256 rgba
	basis -ktx2 -uastc -file "$W/flat-512x256.png" -output_file assets/ktx2/uastc-512x256.ktx2
	flat_png "$W/flat-256x128.png" 256x128 rgba
	basis -basis -file "$W/flat-256x128.png" -output_file assets/basis/etc1s-256x128.basis
	flat_png assets/spine/hero.png 1024x1024 rgba
	flat_png assets/spine/hero_2.png 512x256 rgba
	cat >assets/spine/hero.atlas <<'EOF'
hero.png
size: 1024, 1024
filter: Linear, Linear
pma: true
body
bounds: 0, 0, 512, 512
head
bounds: 512, 0, 256, 256

hero_2.png
size: 512, 256
filter: MipMapLinearLinear, Linear
pma: true
cape
bounds: 0, 0, 512, 256
EOF
	cat >assets/svg/icon.svg <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><circle cx="32" cy="32" r="30" fill="#36c"/></svg>
EOF
	"${FF[@]}" -f lavfi -i "testsrc2=s=32x32:d=0.2:r=10" -c:v gif "${BX[@]}" assets/gif/spinner-32x32.gif
fi

# =====================================================================
if want pixi8-inline; then
	begin pixi8-inline
	mkdir -p assets/{images,audio,video,fonts} "$WORK/p8i"
	W="$WORK/p8i"
	bundle pixi8-inline pixi8 esm "$W/bundle.js"
	# Inline the bundle the way SvelteKit inline builds do.
	awk -v bundle="$W/bundle.js" '
    /__BUNDLE__/ {
      split($0, parts, "__BUNDLE__")
      printf "%s", parts[1]
      while ((getline line < bundle) > 0) printf "%s\n", line
      print parts[2]
      next
    }
    { print }
  ' "$SRC/html/pixi8-inline.html" >index.html
	cp "$SRC/html/app.css" assets/app.css
	"$TOOL" fake-font assets/fonts/fixture.woff2 wOF2 64
	flat_png assets/images/reels-2048x2048.png 2048x2048 rgba
	gradient_png "$W/symbols.png" 1024x512 rgba
	cwebp -quiet -q 75 "$W/symbols.png" -o assets/images/symbols-1024x512.webp
	jpeg assets/images/bg-1280x720.jpg 1280x720
	gated_mono 44100 3 "lt(t,2.4)" "$W/sounds.wav"
	to_m4a "$W/sounds.wav" assets/audio/sounds.m4a 64k -movflags +faststart
	cat >assets/audio/sounds.json <<'EOF'
{
  "src": ["./assets/audio/sounds.m4a"],
  "sprite": {
    "base_loop": [0, 2400, true]
  }
}
EOF
	"${FF[@]}" -f lavfi -i "testsrc2=s=64x64:d=0.4:r=10" -c:v libx264 -preset veryfast -pix_fmt yuv420p "${BX[@]}" assets/video/loader.mp4
	mapfile -t packages < <(bundled_packages pixi8-inline pixi8 esm)
	notices pixi8-inline "${packages[@]}"
fi

# =====================================================================
if want pixi8-webgpu; then
	begin pixi8-webgpu
	mkdir -p assets "$WORK/p8w"
	W="$WORK/p8w"
	bundle pixi8-webgpu pixi8 esm "$W/bundle.js"
	HASH="$(shasum -a 256 "$W/bundle.js" | cut -c1-8)"
	cp "$W/bundle.js" "assets/index-$HASH.js"
	sed "s/__HASH__/$HASH/" "$SRC/html/pixi8-webgpu.html" >index.html
	flat_png "$W/symbols.png" 1024x1024 rgba
	basis -ktx2 -mipmap -file "$W/symbols.png" -output_file assets/symbols-1024x1024.ktx2
	gradient_png assets/frame-1200x800.png 1200x800 rgba
	mono_tone 44100 0.2 "$W/click.wav"
	to_mp3_cbr "$W/click.wav" assets/click.mp3 64k
	mapfile -t packages < <(bundled_packages pixi8-webgpu pixi8 esm)
	notices pixi8-webgpu "${packages[@]}"
fi

# =====================================================================
if want pixi7-aa; then
	begin pixi7-aa
	mkdir -p assets
	bundle pixi7-aa pixi7 iife "$FIXTURES/pixi7-aa/build/main.js"
	cp "$SRC/html/pixi7-aa.html" index.html
	flat_png assets/atlas-1024x1024.png 1024x1024 rgba
	gradient_png assets/panel-1000x600.png 1000x600 rgba
	jpeg assets/tile-512x512.jpg 512x512
	mapfile -t packages < <(bundled_packages pixi7-aa pixi7 iife)
	notices pixi7-aa "${packages[@]}"
fi

# =====================================================================
if want pixi7-opts; then
	begin pixi7-opts
	mkdir -p assets "$WORK/p7o"
	bundle pixi7-opts pixi7 iife "$FIXTURES/pixi7-opts/build/main.js"
	cp "$SRC/html/pixi7-opts.html" index.html
	gradient_png "$WORK/p7o/bg.png" 720x1280 rgb24
	cwebp -quiet -q 75 "$WORK/p7o/bg.png" -o assets/bg-720x1280.webp
	mapfile -t packages < <(bundled_packages pixi7-opts pixi7 iife)
	notices pixi7-opts "${packages[@]}"
fi

# =====================================================================
if want duplicates; then
	begin duplicates
	mkdir -p a b c audio data "$WORK/dup"
	gradient_png a/coin.png 64x64 rgba
	cp a/coin.png b/coin-copy.png
	cp a/coin.png c/coin.png
	gradient_png b/gem.png 48x48 rgba
	mono_tone 22050 0.3 "$WORK/dup/click.wav"
	to_mp3_cbr "$WORK/dup/click.wav" audio/click.mp3 48k
	cp audio/click.mp3 audio/click-old.mp3
	printf '{"lines":20,"rows":5}\n' >data/config.json
	cp data/config.json data/config.backup.json
	: >empty-a.txt
	: >empty-b.txt
	printf '<!doctype html>\n<title>dyno fixture: duplicates</title>\n' >index.html
fi

# =====================================================================
if want broken; then
	begin broken
	mkdir -p ok bad "$WORK/br"
	W="$WORK/br"
	# Two good files, so totals over the readable files are not zero.
	gradient_png ok/valid-128x128.png 128x128 rgba
	mono_tone 22050 0.5 ok/valid-tone.wav
	# One broken file per family. Sources first, then cut or damage them.
	gradient_png "$W/a.png" 64x64 rgba
	head -c 20 "$W/a.png" >bad/png-truncated.png
	jpeg "$W/a.jpg" 64x64
	head -c 20 "$W/a.jpg" >bad/jpeg-no-sof.jpg
	cwebp -quiet -q 75 "$W/a.png" -o "$W/a.webp"
	head -c 20 "$W/a.webp" >bad/webp-truncated.webp
	avif "$W/a.avif" 64x64
	head -c 32 "$W/a.avif" >bad/avif-ftyp-only.avif
	"$TOOL" ktx2-rgba8 "$W/a.ktx2" 64 64 1
	head -c 200 "$W/a.ktx2" >bad/ktx2-truncated.ktx2
	basis -basis -file "$W/a.png" -output_file "$W/a.basis"
	head -c 40 "$W/a.basis" >bad/basis-truncated.basis
	stereo_tone 44100 1 "$W/s.wav"
	to_vorbis "$W/s.wav" "$W/a.ogg" 1004
	head -c "$(($(wc -c <"$W/a.ogg") - 100))" "$W/a.ogg" >bad/vorbis-truncated.ogg
	to_opus "$W/s.wav" "$W/a.opus" 64k
	head -c 40 "$W/a.opus" >bad/opus-truncated-head.opus
	head -c 2048 /dev/zero >bad/mp3-no-sync.mp3
	to_adts "$W/s.wav" "$W/a.aac" 128k
	head -c "$(($(wc -c <"$W/a.aac") - 50))" "$W/a.aac" >bad/adts-truncated.aac
	to_m4a "$W/s.wav" "$W/a.m4a" 128k
	head -c 4096 "$W/a.m4a" >bad/m4a-no-moov.m4a
	head -c 20000 "$W/s.wav" >bad/wav-truncated.wav
	to_webm "$W/s.wav" "$W/a.webm" 64k
	# Cut inside the last cluster, so the track list still says audio.
	head -c "$(($(wc -c <"$W/a.webm") - 100))" "$W/a.webm" >bad/webm-truncated.webm
	printf 'const = ;\nexport default 1\n' >bad/bad-syntax.js
	printf '{"src": ["ok/valid-tone.wav"], "sprite": {"a": [0, 100],\n' >bad/sprite-broken.json
	printf '<!doctype html>\n<title>dyno fixture: broken</title>\n<script src="./bad/bad-syntax.js"></script>\n' >index.html
fi

# =====================================================================
if want zip-input; then
	# Same files as duplicates/build, zipped under one top-level folder.
	[ -d "$FIXTURES/duplicates/build" ] || {
		echo "zip-input needs duplicates first" >&2
		exit 1
	}
	# The scan input is fixtures/zip-input/build.zip, not a folder.
	echo "fixture: zip-input"
	mkdir -p "$FIXTURES/zip-input" "$WORK/zip/game"
	rm -f "$FIXTURES/zip-input/build.zip"
	cp -R "$FIXTURES/duplicates/build/." "$WORK/zip/game/"
	find "$WORK/zip/game" -exec touch -t 202601010000 {} +
	(cd "$WORK/zip" && find game -type f | sort | zip -X -D -q "$FIXTURES/zip-input/build.zip" -@)
fi

# =====================================================================
if want over-budget; then
	# Small files with large decoded sizes, so the audio, stereo and texture
	# rules fire at their default thresholds.
	begin over-budget
	mkdir -p assets/music assets/textures
	# 6 minutes of digital silence, stereo. Vorbis makes this about 20 KB.
	"${FF[@]}" -f lavfi -i "anullsrc=r=44100:cl=stereo:d=360" -c:a pcm_s16le "${BX[@]}" "$WORK/long.wav"
	to_vorbis "$WORK/long.wav" assets/music/loop-stereo.ogg 2001
	# Flat colours, each one different, so no file is a duplicate.
	png assets/textures/big-1-4096x4096.png 4096x4096 rgba "color=c=0x224466"
	png assets/textures/big-2-4096x4096.png 4096x4096 rgba "color=c=0x446622"
	png assets/textures/big-3-4096x4096.png 4096x4096 rgba "color=c=0x662244"
	png assets/textures/big-4-4096x4096.png 4096x4096 rgba "color=c=0x226644"
	png assets/textures/mid-2048x2048.png 2048x2048 rgba "color=c=0x664422"
	png assets/textures/small-1024x1024.png 1024x1024 rgba "color=c=0x442266"
	printf '<!doctype html>\n<title>dyno fixture: over-budget</title>\n' >index.html
fi

echo "done. Check: git status fixtures"
