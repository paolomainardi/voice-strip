# Handoff: live voice processor (macOS / Linux)

This document summarizes the decisions made during the design phase. It is the starting specification: when in doubt, what is written here wins. Deviations must be discussed before they are implemented.

## Goal

A desktop application for macOS and Linux that makes a live microphone **sound professional**: the voice should come out like it went through a well set up broadcast chain, with no audio knowledge required from the user. Use cases: calls, podcasts, streaming, voiceover.

**Noise reduction is one stage of the chain, not the product.** The product is overall voice quality:

- **Tonal balance**: a full, clear voice regardless of the microphone (mud removed, presence added, proximity effect controlled).
- **Dynamics**: consistent level without pumping, natural for speech.
- **Loudness**: hits a known loudness target and never clips.
- **Clean-up of speech artifacts**: sibilance, plosives, room reverb, background noise.
- **Zero configuration**: a short calibration and a preset choice replace manual tuning.

Differentiation versus Krisp, EasyEffects and macOS Voice Isolation: those focus on removing noise. This app delivers podcast-grade sound (EQ, dynamics, loudness, limiting) with zero configuration, cross-platform, no account, no cloud.

### Non-goals (v1)

- Creative or color effects (telephone, radio, robot, pitch). Possibly later, in the advanced view only.
- Acoustic echo cancellation: call apps already do it, and doing it well requires the speaker reference signal. Recommend headphones.
- Music or instrument processing.
- Hosting third-party plugins.

## Quality goals (measurable)

Each goal is verified by automated tests on real voice recordings (see Acceptance criteria).

| Area | Target |
|---|---|
| Loudness | Output within ±1 LU (Loudness Unit) of the preset target, measured as short-term LUFS (Loudness Units relative to Full Scale) per [EBU R128](https://tech.ebu.ch/publications/r128) |
| Peaks | True peak never above -1 dBTP (decibels True Peak) |
| Tonal balance | Long-term output spectrum within a tolerance band around the preset's target voice curve, independent of the input microphone |
| Sibilance | Energy in the sibilant band reduced on "s" and "sh" events, unchanged elsewhere |
| Plosives | Low-frequency bursts on "p" and "b" reduced, sustained low end preserved |
| Noise | Noise floor in pauses reduced, no audible artifacts on voice (musical noise, warbling, word tails cut) |
| Dynamics | No audible pumping; the Narration preset keeps noticeably more dynamic range than Broadcast |
| Latency | Declared latency equals measured latency, within the budgets below |
| CPU | Full chain well under one core on a recent laptop; per-module benchmarks tracked in CI (Continuous Integration) |

## Architecture

The app reads the physical microphone, processes the signal, and writes it to a **virtual microphone** that the user selects in Zoom, Meet, OBS or a DAW (Digital Audio Workstation). This is the only way to work with any application.

- **macOS**: virtual device as an AudioServerPlugIn (HAL, Hardware Abstraction Layer, plugin), written in Rust on the raw Core Audio plug-in interface; [libASPL](https://github.com/gavv/libASPL) (MIT) is a reference, not a dependency. Do not start from BlackHole (GPLv3). Shared memory between the app and the plugin. Requires a signed and notarized installer, since the plugin lives in `/Library/Audio/Plug-Ins/HAL`.
- **Linux**: native [PipeWire](https://pipewire.org) node (`pw_filter`, `pipewire-rs` crate) exposing a virtual source. PulseAudio is covered by the PipeWire compatibility layer.
- **Core**: Rust, 48 kHz, float32, 10 ms frames, lock-free ring buffers between capture and output.

### Cargo workspace

| Crate | Role |
|---|---|
| `dsp` | Pure processing chain, no I/O, testable on files |
| `engine` | Audio thread, ring buffers, routing to virtual mic and headphones |
| `app` | Standalone UI (standard view and advanced view) |
| `plugin` | CLAP / VST3 export of the same chain (later phase) |
| `vdev-macos-driver` | HAL plugin |
| `vdev-linux` | PipeWire node |

## Realtime rules (non-negotiable)

- In the audio callback: no allocations, no locks, no I/O, no logging.
- In debug builds the callback is wrapped with `assert_no_alloc`: a violation fails the tests.
- Parameters from the UI travel through atomics with smoothing. Meter data travels to the UI through lock-free queues. The UI never touches the audio thread.
- Every module declares its latency; total latency is the sum and is shown in the UI.
- SIMD where useful, denormal handling, smoothing on every parameter (no clicks when a control moves).

## Latency budget

Two separate output paths:

| Path | Chain | Estimated latency |
|---|---|---|
| Virtual microphone | Full chain, including neural denoise | about 25 ms |
| Headphone monitoring (optional, off by default) | Reduced chain, no neural denoise | about 4 ms |

- Headphone monitoring is **optional**. When off it must add no processing load.
- Option to include neural denoise in the monitor path too (+20 ms), with a warning: above about 10 ms people hear their own voice as delayed or phasey.
- Monitor path buffer: 64 samples.
- EQ is minimum-phase IIR (Infinite Impulse Response) only, no linear phase. Limiter lookahead 1 to 2 ms. Oversampling only where needed (saturation, true peak detection), minimum phase.

## Processing chain

Fixed order, not user-reorderable. The chain is designed as a whole: each stage assumes the previous ones did their job.

| # | Module | Purpose and notes | In monitor path |
|---|---|---|---|
| 1 | Input | Trim, polarity, high-pass 70 to 120 Hz with proximity-effect compensation | Yes |
| 2 | Denoise | [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) (MIT/Apache) with wet/dry mix, plus a downward expander for pauses. RNNoise as lightweight fallback. Optional light de-reverb | No (optional) |
| 3 | De-plosive | Dynamic compression below 150 Hz, transient-triggered only | Yes |
| 4 | De-esser | Split-band, frequency detected during calibration | Yes |
| 5 | Dynamic EQ | 4 bands acting only when the voice exceeds the target curve (mud, nasality, harshness), plus static shaping toward the preset's target voice curve | Yes |
| 6 | Compressor | Opto-style, program-dependent, parallel mix | Yes |
| 7 | Saturation | Subtle, optional, for density | No |
| 8 | Leveler | Slow AGC (Automatic Gain Control) toward the preset's LUFS target | No |
| 9 | Limiter | True peak, ceiling -1 dBTP | Yes |

Loudness metering uses the `ebur128` crate.

## Calibration

15 seconds on first launch and on request: 5 s of silence to measure the noise floor, 10 s of speech to measure level and timbre. From this the app sets input gain, thresholds, de-esser frequency, and the static EQ correction toward the target voice curve. The result is stored as a voice profile (person plus microphone).

## Presets

Five presets, stored as versioned configuration files (not hardcoded). Each defines a target voice curve, dynamics settings and a loudness target. Calibration adapts each preset to the user's voice and microphone.

| Preset | Character | Key settings | Target |
|---|---|---|---|
| Broadcast | Full, close voice; the "radio" sound | HPF (High-Pass Filter) 80 Hz, proximity effect used for warmth, firm compression, clear presence at 3 to 5 kHz, light saturation, light denoise | -16 LUFS |
| Podcast | Natural and consistent | Moderate compression, mild presence, slow leveler, gentle de-esser, light denoise | -16 LUFS |
| Call | Maximum intelligibility | HPF around 120 Hz, strong denoise, light compression, intelligibility-focused EQ | -18 LUFS |
| Stream | Present and energetic, sits on top of music and game audio | More compression and saturation, stronger presence | -14 LUFS |
| Narration | Intimate and dynamic, minimal intervention; audiobook-ready | Low compression to preserve dynamics, nearly flat tone, peaks at -3 dBTP (ACX), more active de-reverb | -18 LUFS |

The presets are compared with published loudness and voice processing references in [presets.md](presets.md).

## UI

Two views, linked to each other.

**Design reference**: the artboard sources in `design/` are the source of truth for the visual design.

### Standard view (`design/Standard.dc.html`)

Small window (about 420×600), vintage hardware look: cream faceplate, dark frame.

- Two analog VU meters with needles: input (with peak lamp) and output (with LUFS readout).
- Gain reduction LED bar (12 segments).
- Five preset push buttons with lamps, plus a one-line description of the active preset.
- One single knob: Intensity.
- Headphone monitoring toggle switch (off by default) with lamp.
- A/B button (processed / original) and ON / Bypass switch.
- Status line with latencies; link to the advanced view.

### Advanced view (`design/Main.dc.html`)

Large window, dark theme.

- Header: input device, output (virtual mic), latency badges for mic and headphones, A/B, Bypass, link to the compact view.
- Preset selector, voice profile, Recalibrate.
- Spectrum analyzer with highlighted voice band: input, output, EQ action, noise floor before and after.
- Loudness panel: momentary, short-term, true peak, input peak.
- Strip of 9 modules: on/off, macro value, gain reduction meter, latency, "M" badge when the module is in the monitor path.
- Headphone monitoring panel: switch, device, level, denoise in monitor path, active chain and latency.

### Notes on the files in `design/`

They are sources from a design tool and do not open on their own in a browser (they depend on an external runtime, `support.js`). Read them as a specification: layout, colors, fonts, copy, states. The logic at the bottom of each file (`class Component`) describes interactive states and a simulated meter signal; it is not code to reuse. UI copy in the files is in Italian; final copy and localization are to be decided.

Standard view palette: faceplate `#EAE2CE`, ink `#2B2620`, frame `#2A1F17`, VU face `#F3E7C4`, red zone `#B4372A`, amber lamp `#F0A23B`. Fonts: Oswald (labels), IBM Plex Mono (values).
Advanced view palette: background `#0E1013`, panels `#161A1F`, borders `#2A3038`, text `#E9ECEF`, accent `#F0A23B`, headphones `#6BB0F2`. Fonts: IBM Plex Sans, IBM Plex Mono.

### UI technology

Rust on the GPU (wgpu) with iced or vizia, to be chosen through a spike: VU meters at 60 fps, a rotary knob, tray/menubar on both OSes. The same toolkit can be reused in the plugin through [nih-plug](https://github.com/robbert-vdh/nih-plug). Tray and menubar are a separate thin native layer.

## Licensing to verify

- VST3: the bindings used by nih-plug are GPLv3. Decide the distribution model before shipping VST3. Ship CLAP first.
- Test material: only recordings made by the team or with a clear license.

## Development plan

1. **Setup**: workspace, `CLAUDE.md` (pointing to this document and restating the realtime rules), hooks (`cargo fmt`, `clippy -D warnings`, tests), CI.
2. **Feasibility spike**: passthrough from the physical mic to the virtual mic, Linux first, then macOS. No DSP (Digital Signal Processing). Automated loopback latency test.
3. **Offline DSP**: `dsp` crate working on real WAV recordings (quiet room, reverberant room, fan noise, keyboard, sibilant speakers, different microphones). Build stages in this order: high-pass, limiter, compressor, leveler, de-esser, dynamic EQ and target curve, de-plosive, saturation, denoise last.
4. **Realtime engine**: wire the DSP in, optional monitor path, per-module CPU benchmarks in CI.
5. **UI**: standard view first, then advanced view.
6. **Plugin and distribution**: CLAP, then VST3 if licensing allows; notarized pkg and Flatpak.

## Acceptance criteria

Every task closes with a measurable test: latency, loudness, true peak, spectral balance against the target curve, sibilance and plosive reduction, noise floor reduction, CPU, no allocations in the callback, null test with each module at neutral settings. "Sounds better" is never an acceptance criterion for an automated task. Listening evaluations stay human, at the end of each phase, on before/after WAV files generated by the tests.
