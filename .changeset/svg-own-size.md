---
default: minor
---

# Decoded textures count an SVG at its own size

`static.textures.decoded` now counts an SVG whose root element gives an absolute `width` and `height`, such as `width="80" height="40"`, as an RGBA8 image of that size: `w * h * 4`, with Pixi 7's mips for sides that are powers of two. Before, every SVG was left out, so `textures.over_budget` was incomplete for any build with one.

The game can draw an SVG at another size or resolution, so each SVG counted this way is also a gap in the measure's scope (`svg_own_size`). An SVG with only a `viewBox`, or a size in `%` or `em`, has no size of its own and stays out of the total, now with the reason `svg_no_size`. A file named `.svg` with no SVG root element is `invalid`.
