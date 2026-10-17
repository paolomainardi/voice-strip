//! Renders a recording of a real voice through every preset, uncalibrated
//! and calibrated, for listening (`just test-voice`).
//!
//! The recording starts with 5 s of silence (the room), then speech, as the
//! app's calibration expects. Writes one WAV per preset, `report.md` and
//! `index.html` (players to compare each preset with the original) next to
//! the recording. Presets with noise removal run in real time.
//!
//! Usage: render_voice <recording.wav>

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use app_core::calibration::Calibrator;
use app_core::presets::builtin;
use app_core::{PresetId, VoiceProfile, settings_for};
use dsp::util::{Biquad, Coefs};
use dsp::{Chain, ChainSettings, PathKind};

const RATE: u32 = 48_000;
const BLOCK: usize = 480;
const INTENSITY: u8 = 60;
/// Loudness is measured after the leveler has settled.
const SETTLE_S: usize = 15;

struct Render {
    name: String,
    preset: PresetId,
    calibrated: bool,
    y: Vec<f32>,
    missed: u64,
}

fn read(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .into_samples::<f32>()
        .map(|s| s.unwrap())
        .collect()
}

fn write(path: &Path, x: &[f32]) {
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

fn render(x: &[f32], preset: PresetId, profile: &VoiceProfile) -> (Vec<f32>, u64) {
    let denoise = builtin(preset).default_denoise;
    let neural = denoise.map(|_| {
        Box::new(
            denoise_neural::DfnDenoiser::spawn()
                .expect("noise removal model: run just fetch-model"),
        ) as Box<dyn dsp::FrameDenoiser>
    });
    let mut chain = Chain::with_denoiser(PathKind::Main, BLOCK, neural);
    chain.set_settings(&ChainSettings {
        modules: settings_for(preset, INTENSITY, denoise, profile),
        ..ChainSettings::default()
    });
    let mut y = x.to_vec();
    // The neural worker keeps up only in real time: pace the blocks.
    let paced = denoise.is_some();
    let mut next = Instant::now();
    for b in y.chunks_mut(BLOCK) {
        chain.process(b);
        if paced {
            next += Duration::from_millis(10);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
    }
    // Compensate the chain's delay so every file lines up with the original.
    let lat = chain.latency_samples().min(y.len());
    y.drain(..lat);
    y.extend(std::iter::repeat_n(0.0, lat));
    (y, chain.denoise_missed_frames())
}

fn loudness(x: &[f32]) -> (f64, f64, f64) {
    use ebur128::{EbuR128, Mode};
    let mut m = EbuR128::new(1, RATE, Mode::I | Mode::LRA | Mode::TRUE_PEAK).unwrap();
    m.add_frames_f32(x).unwrap();
    (
        m.loudness_global().unwrap(),
        m.loudness_range().unwrap(),
        20.0 * m.true_peak(0).unwrap().log10(),
    )
}

fn db(x: &[f32]) -> f64 {
    let p = x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    10.0 * p.max(1e-20).log10()
}

/// Level through a filter relative to the whole signal, dB.
fn band(x: &[f32], c: Coefs) -> f64 {
    let mut f = Biquad::new(c);
    let y: Vec<f32> = x.iter().map(|&s| f.tick(s)).collect();
    db(&y) - db(x)
}

/// Level of the quietest 10% of 100 ms windows, dB: the room between words.
fn floor_db(x: &[f32]) -> f64 {
    let mut w: Vec<f64> = x.chunks(4_800).map(db).collect();
    w.sort_by(f64::total_cmp);
    w[w.len() / 10]
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: render_voice <recording.wav>");
        std::process::exit(2)
    });
    let input = Path::new(&arg);
    let dir = input.parent().unwrap_or(Path::new("."));
    let x = read(input);

    let mut cal = Calibrator::new();
    cal.feed(&x);
    let calibrated = match cal.result() {
        Some(Ok(p)) => Some(*p),
        Some(Err(e)) => {
            eprintln!("calibration failed: {e}; rendering uncalibrated only");
            None
        }
        None => {
            eprintln!("recording too short to calibrate; rendering uncalibrated only");
            None
        }
    };
    if let Some(p) = &calibrated {
        eprintln!(
            "calibrated: trim {:+.1} dB, noise floor {:.1} dB, sibilance {:.0} Hz, EQ {:?}",
            p.trim_db, p.noise_floor_db, p.sibilance_hz, p.eq_correction_db
        );
    }

    let mut jobs = Vec::new();
    for preset in PresetId::ALL {
        for profile in [Some(VoiceProfile::default()), calibrated]
            .into_iter()
            .flatten()
        {
            let is_cal = profile != VoiceProfile::default();
            let x = x.clone();
            jobs.push(std::thread::spawn(move || {
                let (y, missed) = render(&x, preset, &profile);
                let name = format!(
                    "{}{}",
                    format!("{preset:?}").to_lowercase(),
                    if is_cal { "-calibrated" } else { "" }
                );
                Render {
                    name,
                    preset,
                    calibrated: is_cal,
                    y,
                    missed,
                }
            }));
        }
    }
    eprintln!("rendering {} versions, about a minute...", jobs.len());
    let renders: Vec<Render> = jobs.into_iter().map(|j| j.join().unwrap()).collect();

    let settle = (SETTLE_S * RATE as usize).min(x.len() / 2);
    let mut report = String::from(
        "# Voice Strip on your voice\n\n\
         | Version | Loudness (target) | True peak | Range | Low | Presence | Room between words | Missed frames |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    let row = |r: &mut String, name: &str, target: &str, y: &[f32], missed: u64| {
        let (l, lra, tp) = loudness(&y[settle..]);
        let _ = writeln!(
            r,
            "| {name} | {l:.1} LUFS{target} | {tp:.1} dBTP | {lra:.1} LU | {:.1} dB | {:.1} dB | {:.1} dB | {missed} |",
            band(&y[settle..], Coefs::lowpass(150.0, 0.707)),
            band(&y[settle..], Coefs::bandpass(4_000.0, 1.0)),
            floor_db(&y[settle..]) - db(&y[settle..]),
        );
    };
    row(&mut report, "original", "", &x, 0);
    for r in &renders {
        write(&dir.join(format!("{}.wav", r.name)), &r.y);
        let target = format!(" ({:.0})", builtin(r.preset).target_lufs);
        row(&mut report, &r.name, &target, &r.y, r.missed);
    }
    let _ = writeln!(
        report,
        "\nMeasured after the first {SETTLE_S} s. Low: below 150 Hz; presence: around 4 kHz; \
         both relative to the whole signal. Room between words: the quietest 10% of the \
         speech, relative to its average."
    );
    std::fs::write(dir.join("report.md"), &report).expect("report");

    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Voice Strip on your voice</title>\
         <style>body{font:15px system-ui;max-width:760px;margin:2em auto;padding:0 16px}\
         audio{width:100%}h2{font-size:16px;margin:1.4em 0 .3em}</style>\
         <h1>Voice Strip on your voice</h1><p>Each version is the same take. \
         Players are aligned: compare a preset with the original at the same moment.</p>\
         <h2>Original</h2><audio controls preload=none src=raw.wav></audio>",
    );
    for r in &renders {
        let _ = write!(
            html,
            "<h2>{}{}</h2><audio controls preload=none src=\"{}.wav\"></audio>",
            builtin(r.preset).label,
            if r.calibrated { ", calibrated" } else { "" },
            r.name
        );
    }
    std::fs::write(dir.join("index.html"), html).expect("html");
    println!("{report}");
    println!("Listen: {}", dir.join("index.html").display());
}
