# dyno — Red hot logo pack

The dyno wordmark is drawn from custom geometric letterforms. It heats from cherry red at the **d** to white hot at the **o**, like a tailpipe glowing at the tip.

The **o** is a tachometer: an open 270° gauge with one cut that marks off the redline, and a needle pushed into it. The letters lean forward 10°. The tacho doesn't slant with them: it stays a true circle, tilted onto the same axis so the needle points forward. On its own, the tacho is the dyno mark, used for app icons, favicons and social profiles.

## What's in here

**`svg/`**: master vector files for the wordmark. Letters are outlined shapes, so no fonts are needed.

| File | Use it for |
| --- | --- |
| `dyno-redhot.svg` | **Primary.** Dark backgrounds, 200 px wide and up. Glow and incandescent core. |
| `dyno-redhot-carbon.svg` | The primary logo on a carbon card (960×560). Ready for slides, headers and previews. |
| `dyno-redhot-flat.svg` | Dark backgrounds where the glow won't render, or where the logo is under 200 px. |
| `dyno-redhot-light.svg` | Light backgrounds. A darker ramp keeps the **o** visible on white. |
| `dyno-white.svg` / `dyno-black.svg` | Single colour: photos, merch, embossing, mono print. |

**`png/`**: every wordmark above as a transparent PNG at 512, 1024 and 2048 px wide. The carbon card comes at 960, 1920 and 3840 px.

**`icon/`**: the tacho mark on its own.

| File | Use it for |
| --- | --- |
| `favicon.ico` | Website favicon (16, 32 and 48 px inside one file). |
| `dyno-favicon.svg` | SVG favicon for modern browsers. |
| `favicon-16.png` / `-32.png` / `-48.png` | Single-size favicons. |
| `apple-touch-icon.png` | iOS home screen (180 px). |
| `icon-192.png` / `icon-512.png` | Web app manifest, Android. Safe to use as `maskable`: the mark sits well inside the safe zone. |
| `icon-1024.png` | App Store, Play Store, anywhere that asks for a 1024 square. |
| `dyno-app-icon.svg` | Master for the above: full-bleed carbon square with glow. The platform rounds the corners. |
| `dyno-app-icon-flat.svg` | The same without glow. |
| `dyno-app-icon-light.svg` / `icon-light-512.png` | The icon on paper, for light contexts. |
| `dyno-mark.svg` / `dyno-mark-light.svg` | The mark alone on transparent, for dark / light backgrounds. 512 px PNGs alongside. |
| `dyno-mark-white.svg` / `dyno-mark-black.svg` | Single-colour mark. |

**`social/`**: profile images and banners.

| File | Size | Use it for |
| --- | --- | --- |
| `dyno-avatar-1024.png` | 1024 × 1024 | Master profile image. Sized to survive a circle crop. |
| `dyno-avatar-500.png` | 500 × 500 | GitHub org avatar, Discord, Slack. |
| `dyno-avatar-400.png` | 400 × 400 | X, LinkedIn, Bluesky, YouTube. |
| `dyno-github-social-preview.png` | 1280 × 640 | GitHub repo › Settings › Social preview. |
| `dyno-og-image.png` | 1200 × 630 | `og:image` / `twitter:image` link previews for the website. |
| `dyno-x-header.png` | 1500 × 500 | X profile header. |
| `dyno-linkedin-banner.png` | 1584 × 396 | LinkedIn company banner. The wordmark sits right of centre to clear the profile photo. |

Every PNG in `social/` has an SVG master next to it.

**`terminal/`**: the wordmark in block characters for the CLI.

| File | Size |
| --- | --- |
| `banner.ansi` / `banner.txt` | 57 × 15 characters |
| `banner-small.ansi` / `banner-small.txt` | 38 × 10 characters |

## Picking a version

| Background | Use |
| --- | --- |
| Dark, logo 200 px or wider | `dyno-redhot` |
| Dark, logo smaller | `dyno-redhot-flat` |
| Light | `dyno-redhot-light` |
| One colour only | `dyno-white` / `dyno-black` |
| Square space, or under 72 px | The mark (`icon/`), not the wordmark |

Never put the glow version on a light or busy background. The bloom turns muddy.

## Clear space and minimum size

- Keep clear space on every side equal to **half the height of the o**. That's about 11% of the logo's width.
- The glow files already include padding for the bloom. Measure clear space from the letters, not from the file edge.
- Minimum width on screen: **200 px** for the glow version and **72 px** for flat and single-colour versions. In print: **20 mm**.
- Below 72 px, use the mark. The favicon is a simplified mark with no redline cut and a heavier needle, built for 16–48 px. Use it only at those sizes.

## The mark

- The gauge opening sits at the bottom, tilted 10° with the lean. Don't straighten it or rotate it further.
- In the icon, the heat runs from the bottom-left of the gauge to the top-right, so it still goes cool to hot.
- Give the mark clear space of at least a quarter of its diameter. The icon and avatar files already include it.

## Colour

**Red hot ramp:** runs left to right across the wordmark.

| Name | Hex | Position |
| --- | --- | --- |
| Cherry | `#7A1006` | 0% |
| Red | `#D2230E` | 28% |
| Orange | `#FF6A16` | 52% |
| Amber | `#FFB640` | 74% |
| Incandescent | `#FFE7A6` | 90% |
| White hot | `#FFF8E6` | 100% |

**Light-background ramp:** `#5E0C05` 0% → `#A8190B` 30% → `#D92E0E` 55% → `#F04A0E` 80% → `#F25A12` 100%

**Icon ramp:** `#D2230E` → `#FF6A16` → `#FFB640` → `#FFE7A6` → `#FFF8E6`, bottom-left to top-right. On light: `#7A1006` → `#C21F0C` → `#F04A0E`.

**Terminal ramp:** the same as the red hot ramp but starting at `#B5190B`, so the **d** still reads without the glow.

**Surfaces:** Carbon `#0B0B0E` · Paper `#F2F1EE`

The heat always runs left to right, cool to hot.

## Don't

- Stretch or skew the wordmark, change its 10° lean, or rotate it.
- Slant the tacho with the letters. It stays a true circle.
- Retype "dyno" in a font. Use these files.
- Reverse the gradient or swap in other colours. The redline is a cut, not a red segment.
- Add outlines, drop shadows or extra effects.
- Put the glow version on light or busy backgrounds.

## Website

```html
<link rel="icon" href="/favicon.ico" sizes="48x48">
<link rel="icon" href="/dyno-favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
<meta property="og:image" content="https://dynoapp.io/dyno-og-image.png">
<meta name="twitter:card" content="summary_large_image">
<meta name="theme-color" content="#0B0B0E">
```

`site.webmanifest`:

```json
{
  "name": "dyno",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any maskable" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable" }
  ],
  "theme_color": "#0B0B0E",
  "background_color": "#0B0B0E"
}
```

## GitHub

- **Org avatar:** `social/dyno-avatar-500.png`.
- **Repo social preview:** `social/dyno-github-social-preview.png` (repo › Settings › General › Social preview).
- **README header:** this snippet shows the glow logo in dark mode and the light version in light mode.

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="brand/png/dyno-redhot-1024.png">
  <img alt="dyno" src="brand/png/dyno-redhot-light-1024.png" width="280">
</picture>
```

## Terminal

```sh
cat terminal/banner.ansi
```

To show the banner at startup or on `--version`, print the file's bytes as they are. The `.ansi` files use 24-bit colour, which works in iTerm2, Windows Terminal, VS Code, GNOME Terminal, WezTerm, Kitty and most modern terminals. Print `banner.txt` instead in these cases:

- `NO_COLOR` is set
- stdout isn't a TTY
- the terminal doesn't support truecolor

## Design tools

The SVGs are outlined and font-free. The lean and the tacho's tilt are SVG transforms, which Figma and Illustrator import correctly. The glow is built from SVG filters, which they may flatten or drop. In those tools, either place the PNG, or use `dyno-redhot-flat.svg` and rebuild the glow:

1. Duplicate the letters twice and put both copies under the original.
2. Blur one copy by **3.3%** of the logo's width and the other by **0.9%**.
3. Optional: add a soft white line along the centre of each stroke that fades in from the **n** to the **o**.

## Copies in this repo

This folder is the master. These places hold copies, so update them when a file here changes:

| Copy | From |
| --- | --- |
| `web/public/favicon.ico`, `dyno-favicon.svg`, `apple-touch-icon.png`, `icon-192.png`, `icon-512.png` | `icon/` |
| `web/public/brand/dyno-redhot.svg`, `-flat.svg`, `-light.svg` | `svg/` |
| `web/public/brand/dyno-og-image.png` | `social/` |
| `crates/dyno-cli/src/banner-small.ansi`, `banner-small.txt` | `terminal/`. Above `dyno --version` in a terminal. The runner's Docker build copies only `crates/`, so the CLI keeps its own copy |

The top-level `README.md` shows `png/dyno-redhot-1024.png` and `png/dyno-redhot-light-1024.png` from here.
