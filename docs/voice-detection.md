# Voice detection (Silero VAD)

Voice Strip runs [Silero VAD](https://github.com/snakers4/silero-vad) 6.2.3 to know when someone is speaking. It steers two measurements and never gates audio itself, so it can neither cut a word onset nor add delay.

## What it changes

- **Pause expander.** While nobody speaks, the Denoise stage measures the noise at the expander input and sets the expander threshold above it: room level, plus 6 dB of headroom, plus the preset's margin times Intensity, capped at -30 dBFS. Without a measurement (no detector yet, or digital silence only) the threshold falls back to the calibrated noise floor plus the margin. The expander still opens on the audio level alone.
- **Leveler.** A 400 ms loudness window in which the detector heard no speech at all is ignored, on both the input measurement and the output correction, so steady noise loud enough to pass the EBU R128 gates never moves the gain.

## Room measurement

The detector answers late: a 32 ms chunk, plus queueing. The room tracker accounts for it:

- the noise is measured in 10 ms segments, and a segment counts only 100 ms later, once the detector has reported silence for 200 ms in a row;
- the estimate falls fast (200 ms) and rises slowly (1 s average, and a segment counts at most 3 dB above the estimate), so speech the detector misses is forgotten in the next pause instead of dragging the room up;
- segments below -100 dBFS (a muted interface, a stream starting) are not counted;
- a chain swap (for example a noise removal level change) passes the room to the new chain.

## Integration

```
audio thread (engine)                              vad-worker thread
  every block: push input ── ring (1 s) ──▶ low-pass, 48 → 16 kHz, 512-sample chunks
  set_speech_probability ◀── atomic ◀───────  Silero VAD on tract (~0.04 ms per chunk)
```

- `dsp::VoiceDetector` is the interface: `push` copies samples into an `rtrb` ring (dropping what does not fit) and wakes the worker; `speech_probability` reads an atomic. Both are wait-free and allocation-free (`crates/vad/tests/silero.rs`).
- The engine feeds the raw input to the detector and passes the latest probability to the main chain before each block. The chain turns it into speech or not with Silero's hysteresis (speech from 0.5, silence below 0.35).
- The controller starts the detector on a background thread at launch and hands it to the engine once. If it cannot start, the app works as before, with the calibrated threshold.
- The model is `silero_vad_op18_ifless.onnx` (2.8 MB, MIT), the variant without `If` nodes, which tract loads with the sample rate fixed at 16 kHz. `crates/vad/build.rs` fetches it at commit `5cd7945` and checks its SHA-256; it is embedded with `include_bytes!` and never stored in the repository. `VOICE_STRIP_VAD_MODEL` points the build at a local copy for offline builds.

## Measurements

Private PipeWire, LibriSpeech speech at -26 LUFS with room noise 25 dB below it (`just test-e2e`, speech to pause ratio of the output, higher is quieter pauses):

| Scenario | 0.2.1 | With voice detection |
|---|---|---|
| Input | 24.7 dB | 24.7 dB |
| Broadcast, noise removal off | 20.1 dB | 33.1 dB |
| Broadcast, light | 29.8 dB | 43.2 dB |
| Broadcast, medium | 57.3 dB | 56.8 dB |
| Broadcast, strong | 73.7 dB | 72.0 dB |
| Podcast (light) | 31.8 dB | 45.2 dB |
| Stream (medium) | 54.8 dB | 50.3 dB |
| Call (strong) | 75.4 dB | 75.1 dB |
| Narration (noise removal off) | 20.7 dB | 26.3 dB |

Pauses get 6 to 13 dB quieter wherever noise was audible. With medium and strong noise removal the pauses were already 50 to 75 dB below the speech; there the room-based threshold, set from the tiny residual noise, expands a little less than the fixed one did, a difference far below hearing. Loudness, true peak, delay and each preset's character are unchanged. Without voice detection, the compressor lifted the pauses more than the expander, set from an uncalibrated -70 dBFS floor, turned them down (issue #10).

Unit tests: `crates/dsp/tests/stages.rs` (`expander_learns_the_room`, `room_survives_missed_speech`, `leveler_ignores_noise_without_speech`); `crates/vad/tests/silero.rs` (silence and noise read as no speech; recorded speech is detected in at least 80% of its blocks, pauses read silent in at least 90%).
