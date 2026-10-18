# Neural denoise (DeepFilterNet3, low-latency model)

Stage 2 of the chain removes background noise with DeepFilterNet3 and then applies a downward expander in pauses. This document records where the model comes from, how it is integrated, and the measured timing and quality.

## Model and crate

| Item | Origin | License |
|---|---|---|
| `deep_filter` crate (library `df`) | git dependency on the fork [paolomainardi/DeepFilterNet](https://github.com/paolomainardi/DeepFilterNet), branch `voice-strip`, commit `0feb1e9382284a90e916d3ff4b0bd3acecda5b51` | MIT or Apache-2.0 |
| DeepFilterNet3 low-latency ONNX model, `DeepFilterNet3_ll_onnx.tar.gz` (34.7 MB) | same commit, `models/`, SHA-256 `5998e58e8ba0e09bb76986ef97b84afa065a571ef282d4a1222f341e3251cf3a` | MIT or Apache-2.0 |

Neither is in the repository. The newest `deep_filter` on crates.io (0.2.5, 2022) predates the realtime tract engine, so the crate comes from git at a pinned commit. Cargo does not apply our lints to it.

The fork is upstream v0.5.6 plus a port to `tract` 0.23.9 and `ndarray` 0.17. Upstream has been dormant since 2024 and its `tract` 0.19 is a legacy line (see [sonos/tract#2152](https://github.com/sonos/tract/issues/2152)). The model file is the same as upstream, so its URL and checksum are unchanged.

Measured on a Ryzen 9 9950X:

- Inference per 10 ms frame: p50 0.147 ms on tract 0.19.16, 0.132 to 0.135 ms on 0.23.8 and 0.23.9 (10% faster). p99 about 1.06 ms, now about 0.84 ms (19% faster).
- Output on the same noisy LibriSpeech input: at most 4.7e-6 per sample from the 0.19 output, 106 dB below the signal. This is float rounding with no drift. 0.23.9 is bit-identical to 0.23.8.
- Real speech: pause noise -25.0 dB, speech -0.3 dB, declared latency 1031 samples equals the measured latency, 0 missed frames, also with all cores saturated.
- The `tract` 0.23 GRU state regression ([sonos/tract#2157](https://github.com/sonos/tract/issues/2157)) was fixed by sonos/tract PR #2169 (merged 2026-04-30), before 0.23.8.

### Maintaining the fork

Changes go on the branch `voice-strip` as small commits. Voice Strip pins a full commit SHA, never the branch. To update `tract`, bump the version in the fork, then run the validation: `just test-denoise`, `just test-timing`, and a sample-by-sample comparison of the enhanced output against the previous version. Then bump the SHA in Voice Strip through a pull request. Never rewrite pushed fork history. If upstream resumes, rebase the branch onto upstream and drop what upstream merged.

The model is not embedded in the binary either (`default-model-ll` is off). `denoise_neural::model::locate` finds it, in order:

1. `VOICE_STRIP_MODEL`: a model file the user supplies, used as is.
2. The cached copy, `$XDG_CACHE_HOME/voice-strip/` (or `~/.cache/voice-strip/`) on Linux and `~/Library/Caches/voice-strip/` on macOS, when its SHA-256 matches. A copy that does not match is removed.

`locate` never downloads. `model::Download` fetches the URL above with `curl` on its own thread, reports progress, can be cancelled, and keeps the file only if the SHA-256 matches. The app starts it only after the user agrees in the model dialog, or on `voice-strip ctl model download`; until the model is there, a noise removal level waits in the dialog and `ctl noise` refuses it. `just fetch-model` downloads it from the command line, and `just check` runs it first because the tests need the model. Only the low-latency model is used: the standard DFN3 model adds two frames of lookahead (20 ms).

To update: move the git dependency and the model URL and SHA-256 in `crates/denoise-neural` to a newer tag together, update this file, then rerun the measurements below.

## Integration

Realtime rule: tract allocates during inference, so the model never runs on the audio thread.

```
audio thread (Denoise stage)                    dfn-worker thread
  480-sample frame complete ── submit ──▶ ring ──▶ DfTract::process (~0.15 ms)
  handoff later             ◀── fetch ── ring ◀──
```

- `dsp::FrameDenoiser` is the interface. `submit` and `fetch` only copy samples through `rtrb` ring buffers: wait-free and allocation-free.
- The engine runs the chain on each host block directly, with no re-blocking, so a frame can complete anywhere in a callback. A frame is submitted when it is complete and its result is played a fixed handoff later (`handoff_samples`), chosen so the worker always gets at least 128 samples (2.67 ms) of wall time:
  - callbacks of whole frames (480, 960): one callback;
  - callbacks that divide a frame (240, 160, 48...): one frame, plus whole callbacks up to the budget for very small ones;
  - other sizes (64, 256, 512, 1024, or variable): one frame, plus one callback, plus the budget.
  The controller reads the host block from the engine and rebuilds the chain when it changes.
- Stage latency is `handoff + model delay (fft 960 - hop 480 = 480)`: 20 ms at quantum 480, 24 ms at 64, 28 ms at 256. Constant whatever the wet/dry mix or enable state.
- Frames are counted in flight. A frame that is not delivered in time is replaced by the dry signal delayed by the same latency, with a 2 ms crossfade, and its late result is discarded when it arrives, so a miss never shifts the wet signal against the dry one (`crates/dsp/tests/neural_handoff.rs`, host blocks 1 to 8192 with an injected late frame). Missed frames are counted (`Chain::denoise_missed_frames`).
- On a model error the worker delivers the input delayed by one frame, which keeps the fallback aligned with the dry path.
- The model loads on a background thread when noise removal is turned on; the chain swaps in when it is ready. A chain the engine cannot accept yet (audio not running) is kept and retried every UI tick, never reloaded.
- Queues hold 20 frames, enough for the longest handoff (host block 8192, 18 frames). The worker waits for room instead of dropping a result, and the stage caps its handoff at the queue capacity. The handoff test covers host blocks 1 to 8192 with a bounded queue.
- When the host block grows, the chain is rebuilt with a longer handoff at once; when it shrinks, only after 10 s of stability, because PipeWire changes the quantum as clients come and go. A rebuild reloads the model (follow-up: reuse the loaded denoiser).
- `denoise_atten_limit_db` caps how much noise the model removes: 25 dB by default, 40 dB for the Call preset. A limit keeps a natural room tone instead of digital silence between words.
- Headphone monitor: neural denoise is off by default. The "denoise in headphones" option builds a monitor chain with its own denoiser instance: monitor latency goes from 1.48 ms to 21.48 ms at a 480 host block (measured, `crates/app-core/tests/neural_toggle.rs`), with the same handoff rule as the main path at other sizes.
- Noise removal is optional per preset (Off, Light 12 dB, Medium 24 dB, Strong 40 dB). Turning it off makes the engine swap in a chain without the neural stage, so its 20 ms leave the mic path; the swap happens on the audio thread without allocation and the old chain is dropped on the UI thread. Measured: 1.48 ms off, 21.48 ms on (`crates/app-core/tests/neural_toggle.rs`).

## Measurements

Machine: Arch Linux desktop, AMD Ryzen 9 9950X 16-Core Processor, release build, 2026-10-16. Reproduce with:

```bash
scripts/fetch-testdata.sh
cargo test -p denoise-neural --release -- --nocapture --test-threads=1 --include-ignored
```

| Measure | Result | Criterion |
|---|---|---|
| Inference per 10 ms frame | p50 0.15 ms, p99 1.0 ms, max 1.6 ms | p99 < 2 ms |
| CPU (p50 / period) | about 1.5 % of one core | |
| Audio-side cost per 480-sample block | 0.14 ms worst | < 2 ms |
| Missed frames, idle machine | 0 | 0 |
| Missed frames, all cores saturated (64 busy threads), worker via RealtimeKit | 0 | 0 |
| Declared vs measured stage latency (cross-correlation, real speech) | 1031 vs 1031 samples (includes the 71-sample limiter) | ±2 samples |
| Noise in pauses, fan noise at ~10 dB SNR | -25.0 dB (the attenuation limit) | limit ±3 dB |
| Speech level change | -0.3 dB | within ±2 dB |

Speech material: LibriSpeech (CC BY 4.0), fetched by `scripts/fetch-testdata.sh` into the git-ignored `testdata/` directory and resampled to 48 kHz. LibriSpeech is band-limited to 8 kHz, so it does not exercise the top octave.

### Latency budget of the microphone path

| Part | Latency |
|---|---|
| Neural denoise: worker handoff | 10 ms at quantum 480; 14 ms at 64 (macOS capture); 18 ms at 256 |
| Neural denoise: model (STFT overlap, no lookahead) | 10 ms |
| Limiter lookahead | 1.5 ms |
| **DSP total** | **21.5 ms** at 480, **25.5 ms** at 64, **29.5 ms** at 256 (1.5 ms with noise removal off, at any quantum) |

This fits the budget of about 25 ms at quantum 480 and, within half a millisecond, at the 64-sample callbacks of macOS capture. Voice Strip requests 480 (`node.latency`), and the graph runs at the smallest requested quantum, so larger quanta only happen when the user forces one. PipeWire's default `clock.power-of-two-quantum = true` turns a 480 request into 256 (measured): 29.5 ms with neural denoise on. Setting `clock.power-of-two-quantum = false` in the user's PipeWire configuration restores the 480 quantum.

The 20 ms of model frame and model delay are fixed by DeepFilterNet3's 480-sample hop; only the handoff depends on the callback size. Inference takes at most 1.6 ms on Linux and 1.34 ms on an M4 Pro (`just test-timing`), inside the 2.67 ms worker budget.

## Known limits and next steps

- **Worker scheduling.** Before realtime priority, a run with the CPU saturated by parallel builds missed most frames. The worker now raises itself at start: `SCHED_FIFO` priority 10 when the user has realtime privileges, otherwise RealtimeKit through an in-process D-Bus call (the service PipeWire uses), with `RLIMIT_RTTIME` set to 200 ms only when it was unlimited. Measured with 64 busy threads on all cores: RealtimeKit granted, 0 missed frames (`realtime_under_cpu_load`, ignored by default). macOS will need a time-constraint thread policy.
- **Handoff cost.** Running inference on the audio thread without allocation (preallocated tract state, or a small dedicated implementation of this fixed graph) would remove the 10 ms handoff and bring the DSP path to about 11 ms.
- **Synthetic voices.** DFN treats the synthetic pulse-train test voice as noise (-27 dB). Quality tests must use real speech.
- **Speaker coverage.** The fetched material is one male speaker. Add female and child voices, sibilant speakers and several microphones as the team records licensed material.
