//! End-to-end test of the whole voice path on a live PipeWire graph:
//! recorded speech with room noise plays into a test microphone, the real
//! backend and controller process it into `voice_strip_mic`, and every
//! preset and noise removal level is measured on what comes out.
//!
//! Run it with `just test-e2e`, which starts a private PipeWire server so
//! nothing reaches the desktop's audio, and needs `just fetch-testdata` and
//! `just fetch-model`. Each scenario's output and a report are written to
//! `target/e2e/` for listening.
//!
//! Per scenario it checks that nothing drops out (no missed buffers, no
//! gaps, no non-finite samples), that the loudness settles within 1 LU of
//! the preset's target, that true peaks stay under the limiter ceiling, and that the
//! measured delay matches the one the app reports. Across scenarios it
//! checks that each noise removal level leaves less noise in the pauses and
//! that the presets keep their character: compression, low end, presence.

#![cfg(target_os = "linux")]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::presets::builtin;
use app_core::{Controller, DenoiseLevel, PresetId};
use common::*;
use dsp::Module;
use dsp::util::{Biquad, Coefs};
use engine::RunningBackend;
use vdev_linux::{PipeWireBackend, VIRTUAL_MIC_NAME};

const QUANTUM: u32 = 480;
/// LibriSpeech utterances played in each scenario, with noise-only pauses
/// between them. Loudness is measured on the last one only: the leveler
/// needs about 15 s to settle on the target.
const UTTERANCES: [u32; 4] = [0, 1, 6, 3];
const PAUSE_S: f64 = 1.5;
/// Input speech loudness: a typical, untreated microphone.
const INPUT_LUFS: f64 = -26.0;
/// Room noise level relative to the speech loudness.
const NOISE_DB: f64 = -25.0;
/// Pause edges skipped when measuring noise (speech tails, gate release).
const PAUSE_GUARD_S: f64 = 0.3;
/// Loudness tolerance around the preset target (`docs/HANDOFF.md`).
const LUFS_TOLERANCE: f64 = 1.0;
/// Limiter ceiling (-1 dBTP) plus the measurement's margin.
const MAX_TRUE_PEAK_DBTP: f64 = -0.5;
/// Smallest difference that counts as a preset's character, dB.
const CHARACTER_MARGIN_DB: f64 = 0.5;
/// A run of exact zeros this long inside speech is a dropout.
const MAX_ZERO_RUN: usize = 240;

struct Scenario {
    name: &'static str,
    preset: PresetId,
    /// Overrides the preset's noise removal.
    noise: Option<Option<DenoiseLevel>>,
}

struct Input {
    signal: Vec<f32>,
    /// Sample ranges of speech and of noise-only pauses in `signal`.
    speech: Vec<(usize, usize)>,
    pauses: Vec<(usize, usize)>,
}

#[derive(Debug)]
struct Result {
    name: &'static str,
    target_lufs: f64,
    lufs: f64,
    true_peak_dbtp: f64,
    speech_db: f64,
    pause_db: f64,
    latency_ms: f64,
    reported_ms: f64,
    correlation: f64,
    missed_buffers: usize,
    zero_run: usize,
    finite: bool,
    notice: Option<String>,
    /// Level below 150 Hz and around 4 kHz relative to the whole signal,
    /// dB: the low end and the presence of the voice.
    low_db: f64,
    presence_db: f64,
    /// Compressor gain reduction exceeded 10% of the time during the
    /// settled utterance, dB: what the compressor does on loud syllables.
    comp_gr_db: f64,
}

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e")
}

fn read_wav(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap_or_else(|_| panic!("missing {}: run just fetch-testdata", path.display()))
        .into_samples::<f32>()
        .map(|s| s.unwrap())
        .collect()
}

fn write_wav(path: &Path, x: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).expect("wav");
    for &s in x {
        w.write_sample(s).expect("wav sample");
    }
    w.finalize().expect("wav");
}

fn loudness(x: &[f32]) -> f64 {
    let mut m = ebur128::EbuR128::new(1, RATE, ebur128::Mode::I).unwrap();
    m.add_frames_f32(x).unwrap();
    m.loudness_global().unwrap()
}

fn true_peak_dbtp(x: &[f32]) -> f64 {
    let mut m = ebur128::EbuR128::new(1, RATE, ebur128::Mode::TRUE_PEAK).unwrap();
    m.add_frames_f32(x).unwrap();
    20.0 * m.true_peak(0).unwrap().log10()
}

fn rms_db(x: &[f32], ranges: &[(usize, usize)]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for &(a, b) in ranges {
        for &s in &x[a.min(x.len())..b.min(x.len())] {
            sum += (s as f64).powi(2);
            n += 1;
        }
    }
    10.0 * (sum / n.max(1) as f64).max(1e-20).log10()
}

/// Deterministic room noise: brown-ish rumble plus a little mains hum,
/// with unit RMS.
fn room_noise(n: usize) -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut low = 0.0f64;
    let mut x: Vec<f64> = (0..n)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let white = (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
            low = 0.98 * low + 0.02 * white;
            let hum = (std::f64::consts::TAU * 100.0 * i as f64 / RATE as f64).sin();
            white * 0.3 + low * 4.0 + hum * 0.05
        })
        .collect();
    let rms = (x.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
    x.iter_mut().for_each(|v| *v /= rms);
    x.into_iter().map(|v| v as f32).collect()
}

/// Speech at [`INPUT_LUFS`] with room noise [`NOISE_DB`] below it.
fn input() -> Input {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/speech");
    let pause = (PAUSE_S * RATE as f64) as usize;
    let mut clean = vec![0.0f32; pause];
    let (mut speech, mut pauses) = (Vec::new(), vec![(0, pause)]);
    for u in UTTERANCES {
        let x = read_wav(&dir.join(format!("1272-128104-000{u}.wav")));
        speech.push((clean.len(), clean.len() + x.len()));
        clean.extend(x);
        pauses.push((clean.len(), clean.len() + pause));
        clean.extend(std::iter::repeat_n(0.0, pause));
    }
    let gain = 10f64.powf((INPUT_LUFS - loudness(&clean)) / 20.0) as f32;
    clean.iter_mut().for_each(|s| *s *= gain);
    // Noise RMS relative to the speech loudness (LUFS ~ dBFS RMS for speech).
    let noise_rms = 10f64.powf((INPUT_LUFS + NOISE_DB) / 20.0) as f32;
    let signal = clean
        .iter()
        .zip(room_noise(clean.len()))
        .map(|(s, n)| s + n * noise_rms)
        .collect();
    Input {
        signal,
        speech,
        pauses,
    }
}

/// Ticks the controller, as the app's UI loop does, for `d`.
fn run_for(c: &mut Controller, d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        c.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Applies a scenario and waits until the engine runs the matching chain:
/// with noise removal, until the neural stage has loaded.
fn apply(c: &mut Controller, s: &Scenario, io_ms: f32) {
    c.set_preset(s.preset);
    if let Some(n) = s.noise {
        c.set_neural_denoise(n);
    }
    let want_neural = c.state().neural_denoise.is_some();
    let t = Instant::now();
    loop {
        c.tick();
        let engine_ms = c.state().mic_latency_ms - io_ms;
        if want_neural == (engine_ms > 15.0) {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "{}: chain did not switch (engine {engine_ms:.1} ms)",
            s.name
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Let smoothing, the leveler and the gates settle on the new settings.
    run_for(c, Duration::from_secs(1));
}

/// Lag of `y` against `x` within `max_lag`, and the normalized correlation
/// at that lag.
fn lag(x: &[f32], y: &[f32], max_lag: usize) -> (usize, f64) {
    let n = x.len().min(y.len()).saturating_sub(max_lag);
    let ex: f64 = x[..n].iter().map(|&v| (v as f64).powi(2)).sum();
    let mut best = (0, f64::MIN, 0.0);
    for l in 0..max_lag {
        let (mut acc, mut ey) = (0.0f64, 0.0f64);
        for i in 0..n {
            acc += x[i] as f64 * y[i + l] as f64;
            ey += (y[i + l] as f64).powi(2);
        }
        if acc > best.1 {
            best = (l, acc, ey);
        }
    }
    (best.0, best.1 / (ex * best.2).sqrt().max(1e-12))
}

/// Level of `x` through a filter, relative to its whole level, dB.
fn band_db(x: &[f32], c: Coefs) -> f64 {
    let mut f = Biquad::new(c);
    let y: Vec<f32> = x.iter().map(|&s| f.tick(s)).collect();
    let all = [(0, x.len())];
    rms_db(&y, &all) - rms_db(x, &all)
}

fn longest_zero_run(x: &[f32], ranges: &[(usize, usize)]) -> usize {
    let mut longest = 0;
    for &(a, b) in ranges {
        let mut run = 0;
        for &s in &x[a.min(x.len())..b.min(x.len())] {
            run = if s == 0.0 { run + 1 } else { 0 };
            longest = longest.max(run);
        }
    }
    longest
}

fn shrink(ranges: &[(usize, usize)], guard: usize) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .filter(|(a, b)| b - a > 2 * guard)
        .map(|&(a, b)| (a + guard, b - guard))
        .collect()
}

#[test]
#[ignore = "needs a private PipeWire server: run just test-e2e"]
fn every_preset_and_noise_level_end_to_end() {
    let scenarios = [
        Scenario {
            name: "broadcast",
            preset: PresetId::Broadcast,
            noise: None,
        },
        Scenario {
            name: "podcast",
            preset: PresetId::Podcast,
            noise: None,
        },
        Scenario {
            name: "call",
            preset: PresetId::Call,
            noise: None,
        },
        Scenario {
            name: "stream",
            preset: PresetId::Stream,
            noise: None,
        },
        Scenario {
            name: "narration",
            preset: PresetId::Narration,
            noise: None,
        },
        Scenario {
            name: "broadcast-noise-light",
            preset: PresetId::Broadcast,
            noise: Some(Some(DenoiseLevel::Light)),
        },
        Scenario {
            name: "broadcast-noise-medium",
            preset: PresetId::Broadcast,
            noise: Some(Some(DenoiseLevel::Medium)),
        },
        Scenario {
            name: "broadcast-noise-strong",
            preset: PresetId::Broadcast,
            noise: Some(Some(DenoiseLevel::Strong)),
        },
    ];

    let input = input();
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("target/e2e");
    write_wav(&dir.join("input.wav"), &input.signal);

    let nodes = NullNodes::start(QUANTUM);
    let (handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .with_quantum(QUANTUM)
        .start_pipewire(processor, Some(TEST_SRC), None)
        .expect("backend start");
    assert!(
        running.wait_linked(false, Duration::from_secs(5)),
        "capture link not established: {:?}",
        running.error()
    );
    let mut c = Controller::new(handle);
    assert!(
        c.model_ready(),
        "the noise removal model is missing: run just fetch-model"
    );

    let seconds = input.signal.len() / RATE as usize + 5;
    let reference: Recording = Arc::new(Mutex::new(Rec::with_capacity(seconds)));
    let virtual_mic: Recording = Arc::new(Mutex::new(Rec::with_capacity(seconds)));
    let player = Arc::new(Mutex::new(Player {
        signal: input.signal.clone(),
        ..Player::default()
    }));
    let recorder = Recorder::start(
        QUANTUM,
        player.clone(),
        vec![
            CaptureSpec {
                name: "vs_e2e_ref",
                source: TEST_SRC,
                monitor: false,
                rec: reference.clone(),
            },
            CaptureSpec {
                name: "vs_e2e_vmic",
                source: VIRTUAL_MIC_NAME,
                monitor: false,
                rec: virtual_mic.clone(),
            },
        ],
    );
    let ms = |n: usize| n as f32 * 1000.0 / RATE as f32;
    let (io_main, _) = running.io_latency_samples();
    c.set_io_latency_ms(ms(io_main));
    run_for(&mut c, Duration::from_millis(500));

    let guard = (PAUSE_GUARD_S * RATE as f64) as usize;
    let mut results = Vec::new();
    for s in &scenarios {
        apply(&mut c, s, ms(io_main));
        clear(&reference);
        clear(&virtual_mic);
        start_play(&player);
        let t = Instant::now();
        let settled =
            Duration::from_secs_f64(input.speech[UTTERANCES.len() - 1].0 as f64 / RATE as f64);
        let mut comp_gr = Vec::new();
        while is_playing(&player) {
            assert!(t.elapsed() < Duration::from_secs(seconds as u64 + 10));
            c.tick();
            if t.elapsed() > settled {
                comp_gr.push(c.state().meters.gain_reduction_db[Module::Compressor.index()] as f64);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        comp_gr.sort_by(f64::total_cmp);
        run_for(&mut c, Duration::from_millis(300));

        let (x, y, missed_buffers) = align(&reference, &virtual_mic);
        // Where the signal starts in the reference: before it, silence.
        let start = x
            .iter()
            .position(|&v| v != 0.0)
            .expect("reference is silent");
        let (l, correlation) = lag(&x[start..], &y[start..], 4096);
        let y: Vec<f32> = y[start + l..].to_vec();
        let n = input.signal.len().min(y.len());
        let y = &y[..n];
        write_wav(&dir.join(format!("{}.wav", s.name)), y);

        // Loudness once the leveler has settled.
        let from = input.speech[UTTERANCES.len() - 1].0;
        let result = Result {
            name: s.name,
            target_lufs: builtin(s.preset).target_lufs as f64,
            lufs: loudness(&y[from..]),
            true_peak_dbtp: true_peak_dbtp(y),
            speech_db: rms_db(y, &input.speech),
            pause_db: rms_db(y, &shrink(&input.pauses[1..], guard)),
            latency_ms: ms(l) as f64,
            reported_ms: c.state().mic_latency_ms as f64,
            correlation,
            missed_buffers,
            zero_run: longest_zero_run(y, &input.speech),
            finite: y.iter().all(|v| v.is_finite()),
            notice: c.state().notice.clone(),
            low_db: band_db(&y[from..], Coefs::lowpass(150.0, 0.707)),
            presence_db: band_db(&y[from..], Coefs::bandpass(4000.0, 1.0)),
            comp_gr_db: comp_gr.get(comp_gr.len() * 9 / 10).copied().unwrap_or(0.0),
        };
        eprintln!("e2e: {result:?}");
        results.push(result);
    }

    drop(recorder);
    drop(running);
    drop(nodes);

    let input_speech = rms_db(&input.signal, &input.speech);
    let input_pause = rms_db(&input.signal, &shrink(&input.pauses[1..], guard));
    let report = report(&results, input_speech - input_pause);
    std::fs::write(dir.join("report.md"), &report).expect("report");
    eprintln!("{report}");

    let mut failures = Vec::new();
    let quantum_ms = ms(QUANTUM as usize) as f64;
    for r in &results {
        let mut fail = |what: String| failures.push(format!("{}: {what}", r.name));
        if r.missed_buffers > 0 || r.zero_run > MAX_ZERO_RUN || !r.finite {
            fail(format!(
                "dropouts: {} missed buffers, {} zero samples in a row, finite {}",
                r.missed_buffers, r.zero_run, r.finite
            ));
        }
        if let Some(n) = &r.notice {
            fail(format!("notice: {n}"));
        }
        if (r.lufs - r.target_lufs).abs() > LUFS_TOLERANCE {
            fail(format!("{:.1} LUFS, target {:.1}", r.lufs, r.target_lufs));
        }
        if r.true_peak_dbtp > MAX_TRUE_PEAK_DBTP {
            fail(format!("true peak {:.2} dBTP", r.true_peak_dbtp));
        }
        if (r.latency_ms - r.reported_ms).abs() > quantum_ms {
            fail(format!(
                "delay {:.1} ms measured, {:.1} ms reported (correlation {:.2})",
                r.latency_ms, r.reported_ms, r.correlation
            ));
        }
    }
    // Each noise removal level leaves less noise in the pauses than the one
    // below it, relative to the speech level.
    let snr = |name: &str| {
        let r = results.iter().find(|r| r.name == name).unwrap();
        r.speech_db - r.pause_db
    };
    let levels = [
        "broadcast",
        "broadcast-noise-light",
        "broadcast-noise-medium",
        "broadcast-noise-strong",
    ];
    for w in levels.windows(2) {
        if snr(w[1]) < snr(w[0]) + 1.0 {
            failures.push(format!(
                "{} keeps as much noise as {}: {:.1} vs {:.1} dB speech to pause",
                w[1],
                w[0],
                snr(w[1]),
                snr(w[0])
            ));
        }
    }
    // Each preset shapes the voice as designed (`docs/presets.md`): more
    // compression from Stream down to Narration, less low end in Call, more
    // presence in Stream than in Narration.
    let get = |name: &str| results.iter().find(|r| r.name == name).unwrap();
    let order = ["stream", "broadcast", "podcast", "narration"];
    for w in order.windows(2) {
        let (a, b) = (get(w[0]), get(w[1]));
        if a.comp_gr_db < b.comp_gr_db + CHARACTER_MARGIN_DB {
            failures.push(format!(
                "{} compresses no more than {}: {:.1} vs {:.1} dB",
                a.name, b.name, a.comp_gr_db, b.comp_gr_db
            ));
        }
    }
    for r in &results {
        if r.comp_gr_db < CHARACTER_MARGIN_DB {
            failures.push(format!(
                "{}: compressor idle ({:.1} dB)",
                r.name, r.comp_gr_db
            ));
        }
    }
    if get("call").low_db > get("broadcast").low_db - 3.0 * CHARACTER_MARGIN_DB {
        failures.push(format!(
            "call keeps as much low end as broadcast: {:.1} vs {:.1} dB",
            get("call").low_db,
            get("broadcast").low_db
        ));
    }
    if get("stream").presence_db < get("narration").presence_db + CHARACTER_MARGIN_DB {
        failures.push(format!(
            "stream has no more presence than narration: {:.1} vs {:.1} dB",
            get("stream").presence_db,
            get("narration").presence_db
        ));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn report(results: &[Result], input_snr: f64) -> String {
    let mut s = String::from(
        "# Voice Strip end-to-end report\n\n\
         | Scenario | LUFS (target) | True peak | Speech to pause | Delay (reported) | Correlation | Low | Presence | Compressor GR |\n\
         |---|---|---|---|---|---|---|---|---|\n",
    );
    for r in results {
        let _ = writeln!(
            s,
            "| {} | {:.1} ({:.0}) | {:.1} dBTP | {:.1} dB | {:.1} ms ({:.1} ms) | {:.2} | {:.1} dB | {:.1} dB | {:.1} dB |",
            r.name,
            r.lufs,
            r.target_lufs,
            r.true_peak_dbtp,
            r.speech_db - r.pause_db,
            r.latency_ms,
            r.reported_ms,
            r.correlation,
            r.low_db,
            r.presence_db,
            r.comp_gr_db
        );
    }
    let _ = writeln!(
        s,
        "\nInput: speech at {INPUT_LUFS:.0} LUFS, {input_snr:.1} dB speech to pause. \
         Recordings: `input.wav` and one WAV per scenario next to this file."
    );
    s
}
