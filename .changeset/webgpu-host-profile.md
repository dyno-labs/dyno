---
default: minor
---

# WebGPU is on in the Reels host profile

The reference iPhone runs iOS 26, whose Safari has WebGPU on, so the `reels` host profile is now version 2 with WebGPU on. The static scan models a build that asks for WebGPU as WebGPU, not WebGL. The Chromium run gives the game a WebGPU adapter only when the host profile has WebGPU. On Linux, every Chromium run now uses SwiftShader's Vulkan for its graphics, so a WebGPU canvas can draw. On a Mac, Chrome uses the Mac's GPU, which gives WebGPU whatever the profile says. A result made with version 2 has a new `result_hash`, even for a build that does not use WebGPU. A game that draws with WebGPU gets its texture measures as unsupported, since the probe sees only WebGL uploads.
