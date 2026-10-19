# Changelog

All notable changes to Voice Strip are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `just upgrade` moves a clone to the latest release, rebuilds, reinstalls the macOS driver only when it changed, and restarts the app.
- The app shows when a newer release exists, in both views and the tray or menu bar, checking GitHub once a day.
- `voice-strip ctl updates on|off` turns the update check on or off, and `ctl status` reports it.

### Fixed

- A notice in the standard view footer no longer runs over CALIBRATE.

## [0.3.1] - 2026-10-19

### Added

- Cmd+W on macOS and Ctrl+W on Linux hide the window while the app keeps running.
- `voice-strip ctl hide` hides the window.
- `voice-strip ctl status` reports the noise removal and voice detection run times and the frames noise removal missed.

### Changed

- The Show window item in the tray and menu bar, and a click on the Linux tray icon, toggle the window; the item reads Hide window while it is shown.

### Fixed

- Noise removal switched itself off on macOS after 15 to 30 s: it now tells macOS its deadline, so the cores keep up.
- Calibration no longer reports a quiet room as too noisy: rumble, DC offset and single clicks during the silence no longer count as noise.

## [0.3.0] - 2026-10-18

### Added

- Voice detection with the Silero VAD model, built into the app and always on.
- The pause expander learns the room noise while you are silent, so pauses get quieter without calibration.
- A VOICE lamp in the input meter that lights while you speak.
- The advanced view shows the AI models at work: speech probability, room noise and run time.
- Calibrations are saved per microphone and load back when that microphone is selected.
- An amber lamp on CALIBRATE, and a "not calibrated" voice profile, until the microphone is calibrated.
- `voice-strip ctl status` reports the voice detector, speech probability, room noise and calibration.
- A "First run: calibrate" section and an "AI models" chapter in the README.

### Changed

- The leveler measures loudness only while you speak.
- The headphone switch in the standard view is labelled MONITOR.

### Fixed

- Without noise removal, pauses were left noisier relative to the speech than at the input.

## [0.2.1] - 2026-10-18

### Added

- A weekly CI check that fails when tract or upstream DeepFilterNet has a release to review.

### Changed

- Noise removal runs on tract 0.23.9 instead of 0.19, with 10 to 19% faster inference and the same output.

### Fixed

- The inference timing test loads the downloaded model.

## [0.2.0] - 2026-10-17

### Added

- Character gauges under the presets showing how much the selected preset compresses, brightens and keeps warmth, following the Intensity knob.
- Preset tooltips that say what each preset is for and what it does.
- `just record-voice` and `just test-voice` to hear every preset on your own voice, calibrated and not, on a local listening page.
- `just test-e2e`: every preset and noise removal level on noisy speech through a private PipeWire server, checking loudness, true peak, delay, dropouts, noise in pauses and each preset's character.
- `docs/presets.md`, comparing the presets with Apple Podcasts, AES TD1008, EBU R 128, Spotify and ACX.

### Changed

- Broadcast, Podcast and Narration sound clearly different: Broadcast compresses firmly with more presence, air and saturation; Podcast stays natural with moderate compression; Narration is light and nearly flat.
- Stream compresses harder and brighter than Broadcast.
- Narration limits peaks at -3 dBTP, as ACX asks for audiobooks.
- Broadcast and Podcast start with light noise removal.
- The Intensity knob has a mark every 10, so every round value points at one.
- The README links DeepFilterNet3 to its repository and paper, and has a table of contents and an install chapter per system.

### Fixed

- The leveler no longer overshoots the loudness target by about 2 LU when the room is noisy.
- Without the noise removal model, the app starts with noise removal off instead of showing a level it cannot apply.

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

[Unreleased]: https://github.com/paolomainardi/voice-strip/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/paolomainardi/voice-strip/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/paolomainardi/voice-strip/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/paolomainardi/voice-strip/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/paolomainardi/voice-strip/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/paolomainardi/voice-strip/releases/tag/v0.1.0
