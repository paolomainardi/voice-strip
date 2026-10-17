# egui frontend: proof of concept

Branch `feat/egui-poc`. Question: can one egui (eframe) frontend replace both the GTK frontend and the planned native macOS frontend?

## What the POC is

Originally `crates/app-egui`, next to `app-gtk`. After the POC it became `crates/app` (binary `voice-strip`) and GTK was removed.

- `painter.rs`: `ui_kit::Painter` on epaint. Paths are flattened in view coordinates (arcs, cubics, rounded rects), transformed by a 2D affine stack, and emitted as epaint shapes grouped by clip rectangle. Text goes through egui `LayoutJob` with `extra_letter_spacing`, so CSS letter spacing carries over.
- `fonts.rs`: the bundled OFL fonts, one egui family per weight. Oswald is only shipped as a variable font, and egui selects faces by family name, so static Regular, Medium and SemiBold instances were generated with `fonttools varLib.instancer` (allowed by the OFL).
- `main.rs`: eframe app with the same `ui-kit` views, `app-core` controller and audio backends as the GTK app. Borderless, transparent, fixed-size window that follows the active view; drag on bare canvas moves the window; tooltips and the device chooser are egui popups.
- eframe/egui **0.36.2** (0.37.0 was released one day before the POC; AGENTS.md avoids releases younger than five days). MIT or Apache-2.0.

Run it: `just start` or `cargo run --release -p app -- [--backend sim] [--advanced] [--input NODE]`.

## Results

Measured on an Arch Linux desktop (AMD Ryzen 9 9950X 16-Core Processor, AMD Radeon RX 7900 XTX, Hyprland, 3440x1440 at scale 1), 2026-10-16.

| | GTK (Cairo/Pango) | egui |
|---|---|---|
| Visual fidelity | reference | near pixel-identical in both views (side-by-side captures); text renders slightly heavier |
| Standard view, CPU time to build a frame | 0.65 ms (2x, Cairo raster) | 0.07 ms p50, 0.15 ms p99 (shape building; rasterisation on the GPU) |
| Advanced view | 1.75 ms at 1x, 3.6 ms at 2x | 0.16 ms p50, 0.25 ms p99 |
| Process CPU, standard view at 60 fps | 13 % (with the real PipeWire chain) | 5 % (simulated backend) |
| Borderless transparent canvas window | yes | yes (eframe viewport options) |
| Window follows the view size | yes | yes; on Wayland a fixed-size window has to be resized through min/max bounds |
| Platforms from one code base | Linux only; macOS needs a second frontend | Linux and macOS (wgpu/glow, winit) |
| Plugin UI reuse (CLAP/VST3 via nih-plug) | no | yes (nih_plug_egui) |
| Accessibility | one canvas, not exposed | AccessKit available; controls still need to be described to it |

The CPU numbers are not a strict comparison: the GTK instance ran the real audio chain on PipeWire, the egui one the simulated backend.

## Gaps found

- **Rounded clips** are rectangle clips in epaint. Nothing visible depends on them today (the VU needle stays inside its face).
- **Concave fills**: epaint fills convex polygons; the current views only fill convex shapes, but a future concave shape would need tessellation (lyon) or splitting.
- **Text weight** looks a bit heavier than Pango at small sizes. Acceptable at scale 1; to be checked on a HiDPI Mac.
- **Tray** stays a separate thin layer per OS (ksni on Linux, NSStatusItem on macOS), as in the handoff.
- Not done in the POC: screenshot mode, accessibility nodes, keyboard focus, the macOS build itself.

## Decision

Adopted: egui is the only desktop frontend, rendered with wgpu (Vulkan here on an AMD Radeon RX 7900 XTX, Metal on macOS). It now also has the tray icon, screenshot mode and every command-line option the GTK app had.

## Original recommendation

Move to egui for both platforms. It reproduces the design with the same `ui-kit` code, costs an order of magnitude less CPU per frame than the Cairo path, removes the planned AppKit/Core Graphics frontend, and opens the plugin UI path. Next steps: port the remaining GTK features (screenshot mode, tray wiring), describe controls to AccessKit, then build and test it on macOS.
