#![allow(dead_code)]

use std::f32::consts::{PI, TAU};

use dsp::{Chain, ChainSettings, PathKind, SAMPLE_RATE};

pub const FS: f32 = SAMPLE_RATE as f32;

/// Xorshift noise in [-1, 1).
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

pub struct Reso {
    a1: f32,
    a2: f32,
    g: f32,
    y1: f32,
    y2: f32,
}

impl Reso {
    pub fn new(freq: f32, bw: f32) -> Self {
        let r = (-PI * bw / FS).exp();
        let th = TAU * freq / FS;
        Self {
            a1: 2.0 * r * th.cos(),
            a2: -r * r,
            g: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Speech-like signal: glottal pulses with pitch drift through three
/// formants, ~4 Hz syllables, 1.2 s pause every 5 s, slow level drift.
/// `level` scales the result. Deterministic.
pub fn speech(seconds: f32, level: f32, seed: u64) -> Vec<f32> {
    let n = (seconds * FS) as usize;
    let mut out = vec![0.0; n];
    let mut noise = Noise::new(seed);
    let mut phase = 0.0f32;
    let mut f1 = Reso::new(650.0, 90.0);
    let mut f2 = Reso::new(1700.0, 120.0);
    let mut f3 = Reso::new(2800.0, 180.0);
    for (i, x) in out.iter_mut().enumerate() {
        let t = i as f32 / FS;
        let f0 = 125.0 + 25.0 * (t * 0.9).sin() + 10.0 * (t * 3.1).sin();
        phase += f0 / FS;
        let pulse = if phase >= 1.0 {
            phase -= 1.0;
            1.0
        } else {
            0.0
        };
        let src = pulse + 0.03 * noise.next();
        let syll = (0.5 - 0.5 * (TAU * 3.7 * t).cos()).powf(1.5);
        let sentence = if (t % 5.0) < 3.8 { 1.0 } else { 0.0 };
        let drift = 1.0 + 0.4 * (t * 0.31).sin();
        let v = f1.tick(src) + 0.7 * f2.tick(src) + 0.4 * f3.tick(src);
        *x = level * drift * syll * sentence * v * 8.0;
    }
    out
}

/// Pure sine.
pub fn sine(freq: f32, amp: f32, seconds: f32) -> Vec<f32> {
    (0..(seconds * FS) as usize)
        .map(|i| amp * (TAU * freq * i as f32 / FS).sin())
        .collect()
}

pub fn rms_db(x: &[f32]) -> f32 {
    let ms = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    (10.0 * ms.max(1e-30).log10()) as f32
}

pub fn integrated_lufs(x: &[f32]) -> f64 {
    let mut m = ebur128::EbuR128::new(1, SAMPLE_RATE, ebur128::Mode::I).unwrap();
    m.add_frames_f32(x).unwrap();
    m.loudness_global().unwrap()
}

pub fn true_peak_dbtp(x: &[f32]) -> f64 {
    let mut m = ebur128::EbuR128::new(1, SAMPLE_RATE, ebur128::Mode::TRUE_PEAK).unwrap();
    m.add_frames_f32(x).unwrap();
    20.0 * m.true_peak(0).unwrap().log10()
}

/// Scales a signal so its integrated loudness is `lufs`.
pub fn normalize(x: &mut [f32], lufs: f64) {
    let g = 10f64.powf((lufs - integrated_lufs(x)) / 20.0) as f32;
    for v in x.iter_mut() {
        *v *= g;
    }
}

/// Runs `x` through a main chain with `settings`, in 480-sample blocks.
pub fn run_chain(settings: &ChainSettings, x: &[f32]) -> (Vec<f32>, usize) {
    let mut chain = Chain::new(PathKind::Main, dsp::BLOCK_MAIN);
    chain.set_settings(settings);
    let mut out = x.to_vec();
    for block in out.chunks_mut(dsp::BLOCK_MAIN) {
        chain.process(block);
    }
    (out, chain.latency_samples())
}

/// Settings with only `module` enabled.
pub fn only(module: dsp::Module, mut s: ChainSettings) -> ChainSettings {
    s.enabled = [false; 9];
    s.enabled[module.index()] = true;
    s
}

/// Writes a mono 48 kHz float WAV under `target/dsp-wav/` for listening.
pub fn write_wav(name: &str, x: &[f32]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/dsp-wav");
    std::fs::create_dir_all(&dir).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for &v in x {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}
