# DX12 rendering compared with Vulkan

Rustcraft draws with wgpu 30. It tries Vulkan first, and falls back to DX12 only if Vulkan
offers no hardware adapter compatible with the window (`apps/client/src/gpu_backend.rs`).
`RUSTCRAFT_BACKEND=vulkan|dx12` forces an API.

## Adapter choice

- A single Vulkan + DX12 instance is not enough: wgpu 30 sorts adapters by **type** only
  (discrete, integrated, other, virtual, software), and the API order only breaks ties.
  A card classed "other" by Vulkan and "discrete" by DX12 would therefore end up on DX12, and both
  APIs would be initialised on every launch.
- So we try each API alone, in order. The first one that yields a hardware adapter wins. A software
  adapter (WARP, lavapipe) is used only as a last resort.
- DX12 shader compiler pinned to **FXC**. By default, wgpu picks `dxcompiler.dll` if it finds one
  on the PATH. The result would then depend on the machine, and the 1.8.2502 DLL from the Windows
  SDK **crashes** on debug labels containing non-ASCII characters (seen on 17/10).
  `WGPU_DX12_COMPILER=dxc` remains available for experiments; labels are then no longer
  passed to the driver.

## Optional features

The log prints a `GPU features: …` line at startup.

| Feature | Use | Fallback | DX12 (RTX 5080) |
|---|---|---|---|
| `INDIRECT_FIRST_INSTANCE` + indirect execution | batched terrain draws | one draw per run (`RUSTCRAFT_INDIRECT=0`) | yes; frame identical to the fallback, up to noise |
| `TIMESTAMP_QUERY` | GPU time in the log | GPU time absent | yes |
| `POLYGON_MODE_LINE` | wireframe (`RUSTCRAFT_WIRE=1`) | filled rendering | yes, seen on screen |

DX12 pitfall: we turn off wgpu's validation of indirect calls (`VALIDATION_INDIRECT_CALL`).
Without it, wgpu does not copy the start instance of indirect commands into the shader
constants. A shader drawn indirectly must therefore **never** read `@builtin(instance_index)`.
The terrain reads its faces through an instance buffer, which DX12 does offset correctly by itself.

## Pixel-by-pixel comparison (17/10/2026)

Method:

- private official server, flat world, `tick freeze`;
- same position and same view for each scene;
- capture after 20 s, **two launches per API**;
- only the pixels that are stable between the two launches of the same API count (the rest is
  launch-to-launch noise: vignette, torch flames, real-time animations);
- `textureFiltering:0`, except the "anisotropic" row.

Difference = maximum value over the three channels, over 1600 × 900 pixels.

| Scene | Stable pixels where DX12 differs by more than 1 | More than 8 | Max difference |
|---|---|---|---|
| Title screen (frozen panorama, blur, buttons) | 0 | 0 | 0 |
| Daytime world (terrain, water, glass, leaves, entities, wither, dragon, clouds) | 228 (0.016 %) | 178 | 157 |
| Night world (stars, clouds) | 605 (0.047 %) | 34 | 67 |
| Water seen up close | 2,619 (0.43 %) | 0 | 2 |
| Underwater (water fog) | 9,900 (0.70 %) | 0 | 2 |
| Wither and dragon up close, sky, clouds | 3 | 3 | 30 |
| Nether (fog, sky) | 250 (0.018 %) | 0 | 2 |
| End (End sky, crystals) | 191 (0.017 %) | 0 | 2 |
| Spyglass (overlay, zoom) | 0 | 0 | 0 |
| Inventory (player model) | 1,036 | 388 | 194 |
| Daytime world, **anisotropic filtering** (`textureFiltering:2`) | 8,382 (0.58 %) | 14 | 246 |

Reading:

- Differences of 1 or 2: rounding in the shaders compiled by FXC (HLSL) versus SPIR-V. Invisible.
- Isolated large differences (day, night): pixels on the **edge of the clouds**, which flip to one
  side or the other of the edge. The cloud vertices are far away (hundreds of blocks); the last
  decimal of the matrix × vertex product is not the same depending on the compiler.
- Inventory: arms of the model. The idle animation follows the player's age. DX12 shows its first
  frame about 0.6 s later (FXC compilation), so the player enters the world later, and
  the capture lands at a different instant of the animation. This is not a rendering difference.
- Anisotropic filtering: differences of 1 to 8 on distant terrain. The sampler is described the
  same way on both APIs. It is the driver that filters differently, and no API controls it. Without
  anisotropy, the difference disappears.
- Particles: their position is drawn at random, so no pixel-by-pixel comparison is possible.
  Side-by-side view: same sprites, same colours, same sizes (flame, heart, `end_rod`, spells
  from the effect cloud).
- Derivatives: `dpdxFine`/`dpdyFine` in the terrain. The Vulkan frame is unchanged (the Vulkan
  driver already used fine derivatives), and the choice no longer depends on the driver.
- Validation: wgpu's own validation, always on, reports no error in these scenes (it would
  stop the client). The Vulkan and D3D12 validation layers are not installed on the test
  machine. `RUSTCRAFT_GPULOG=warn` with `WGPU_VALIDATION=1` will show their messages
  once they are.

## Cost

On the bench (machine not quiet, indicative figures, not a measurement):

- first frame on DX12 around 1.4 to 1.6 s, against 0.8 s on Vulkan (FXC compilation);
- CPU time per frame about twice as high on DX12 (0.20 ms against 0.10 ms).

Vulkan remains the default API.
