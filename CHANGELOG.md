# Changelog

All notable changes to Voice Strip are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-17

### Added

- Nine-stage voice chain: input filter, neural noise removal, de-plosive, de-esser, static and dynamic EQ, compressor, saturation, leveler and true-peak limiter.
- Broadcast, Podcast, Call, Stream and Narration presets with an Intensity knob and four noise removal levels.
- 15 s calibration to the microphone and room.
- Virtual microphone through PipeWire on Linux and a Core Audio driver on macOS.
- Low-latency headphone monitor.
- Standard and advanced views rendered on the GPU.
- Tray icon on Linux and menu bar item on macOS.
- `voice-strip ctl` remote control and an Omarchy bar widget.
- Noise removal model download that asks for consent first and checks the SHA-256.
- Linux x86_64 release binary.

[Unreleased]: https://github.com/paolomainardi/voice-strip/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/paolomainardi/voice-strip/releases/tag/v0.1.0
