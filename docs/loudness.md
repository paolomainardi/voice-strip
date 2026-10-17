# Loudness and peak measurements

Full main chain (all nine stages except the neural denoiser) on real speech: eight LibriSpeech utterances (CC BY 4.0, one speaker, about 50 s) with 0.5 s pauses between them, input trim set as calibration sets it (speech at -23 LUFS before the dynamics). Measured with the independent `ebur128` crate. Reproduce:

```bash
scripts/fetch-testdata.sh
cargo test -p dsp --release --test stages real_speech -- --ignored --nocapture
```

Output files for listening: `target/dsp-wav/real_chain_14.wav`, `real_chain_16.wav`, `real_chain_18.wav`.

| Target | Integrated | True peak | Short-term deviation p50 | p90 | Windows within ±1 LU |
|---|---|---|---|---|---|
| -14 LUFS (Stream) | -14.19 LUFS | -1.96 dBTP | 0.88 LU | 3.21 LU | 57 % |
| -16 LUFS (Broadcast, Podcast) | -15.63 LUFS | -1.96 dBTP | 0.67 LU | 2.41 LU | 63 % |
| -18 LUFS (Call, Narration) | -17.50 LUFS | -1.97 dBTP | 0.74 LU | 2.43 LU | 64 % |

Measured on an Arch Linux desktop (AMD Ryzen 9 9950X 16-Core Processor), 2026-10-16.

## What the test asserts

- Integrated loudness within ±1 LU of the target.
- True peak at or below -1 dBTP (also verified on adversarial signals: white noise, 16 kHz sines at five phases, fs/4 at 45 degrees, burst trains, all 20 dB hot).
- Median short-term (3 s window) deviation at most 1 LU.

## Open decision: the short-term criterion

The handoff asks for "output within ±1 LU of the preset target, measured as short-term LUFS". On real speech the median 3 s window meets it, but about a third of the windows do not. Those are windows that contain pauses: the leveler deliberately holds its gain during pauses (gating), so it does not pump noise up, and a 3 s window that is half silence reads several LU low. A faster leveler (tracking twice as fast, twice the slew rate) did not change the result, which confirms the deviation comes from the material, not from the leveler.

Options:

1. Keep the spec as integrated ±1 LU plus median short-term ±1 LU (what the tests assert now).
2. Measure short-term only on windows that are entirely speech (needs a voice activity mask in the test).
3. Make the dynamics act faster on level drops, at the cost of pumping and raised noise in pauses.

Option 1 or 2 is recommended; option 3 trades away naturalness.

## Notes

- Limiter: 96-tap true-peak detector, 1 ms attack ramp, 1.5 ms total lookahead, 1 dB internal margin below the ceiling. Output true peak lands about 1 dB under the ceiling on speech (-1.96 dBTP for a -1 dBTP ceiling).
- The synthetic pulse-train voice used by the fast tests has an unrealistic crest factor, so the -14 LUFS target is only tested on real speech.

## Tonal balance across microphones

Same LibriSpeech voice through two simulated microphones, a bassy and dark one (+6 dB low shelf at 200 Hz, -4 dB high shelf at 6 kHz) and a thin and bright one (-6 dB at 300 Hz, +4 dB at 4 kHz), each calibrated, Podcast preset at Intensity 60. Long-term level of the four EQ bands relative to the full band (`crates/app-core/tests/tonal_balance.rs`):

| | Largest band difference between the two microphones |
|---|---|
| Before processing | 9.3 dB |
| After calibration and processing | 4.8 dB |

The remaining difference sits almost entirely in the air band (10 kHz). LibriSpeech is band-limited to 8 kHz, and the target curve values in `calibration::TARGET_CURVE_DB` are initial estimates, so both microphones hit the ±4 dB correction cap there. Applying 80 % of the correction with a ±6 dB cap only moved the result to 4.5 dB, so the conservative 50 % / ±4 dB stays. Next step: derive the target curve from the team's licensed full-band recordings on several microphones.

## Dynamics per preset

Same real-speech material, calibrated trim, Intensity 60 (`crates/app-core/tests/dynamics.rs`). EBU loudness range (LRA):

| | Integrated | LRA |
|---|---|---|
| Input | | 4.5 LU |
| Broadcast | -15.6 LUFS | 2.4 LU |
| Narration | -17.4 LUFS | 3.5 LU |

Narration keeps more of the speaker's dynamics because its leveler follows level changes at a quarter of the Broadcast speed (`leveler_speed` 0.25; Podcast 0.6; Broadcast, Call and Stream 1.0). Before this setting both presets measured about 2.4 LU, which did not meet the handoff requirement.
