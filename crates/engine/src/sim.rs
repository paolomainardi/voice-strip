//! Simulated backend: feeds a synthetic speech-like signal through the
//! processor on a plain thread every 10 ms. Lets the UI run without audio
//! hardware. Not realtime-grade; for development and tests only.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backend::{AudioBackend, BackendError, DeviceInfo, RunningBackend};
use crate::processor::AudioProcessor;

#[derive(Default)]
pub struct SimBackend;

impl AudioBackend for SimBackend {
    fn list_inputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![DeviceInfo {
            id: "sim-voice".into(),
            label: "Simulated voice".into(),
        }])
    }

    fn list_outputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![DeviceInfo {
            id: "sim-null".into(),
            label: "Simulated headphones".into(),
        }])
    }

    fn start(
        &mut self,
        mut processor: AudioProcessor,
        _input: Option<&str>,
        _monitor_output: Option<&str>,
    ) -> Result<Box<dyn RunningBackend>, BackendError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let thread = std::thread::Builder::new()
            .name("sim-audio".into())
            .spawn(move || {
                let mut voice = SpeechLike::new(1);
                let mut buf = vec![0.0f32; dsp::BLOCK_MAIN];
                let mut out = vec![0.0f32; dsp::BLOCK_MAIN];
                let period = Duration::from_millis(10);
                let mut next = Instant::now();
                processor.set_host_block_size(Some(dsp::BLOCK_MAIN));
                processor.set_running(true);
                while !stop_t.load(Ordering::Relaxed) {
                    voice.fill(&mut buf);
                    processor.process(&buf, &mut out, None);
                    next += period;
                    if let Some(d) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(d);
                    }
                }
                processor.set_running(false);
            })
            .map_err(|e| BackendError(e.to_string()))?;
        Ok(Box::new(SimRunning {
            stop,
            thread: Some(thread),
        }))
    }
}

struct SimRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RunningBackend for SimRunning {
    fn virtual_device_name(&self) -> &str {
        "Voice Strip (simulated)"
    }

    fn io_latency_samples(&self) -> (usize, usize) {
        (dsp::BLOCK_MAIN, dsp::BLOCK_MONITOR)
    }
}

impl Drop for SimRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Deterministic speech-like generator: a glottal pulse train with slow
/// pitch drift through two formant resonators, gated by ~4 Hz syllables
/// and occasional pauses, plus a low noise floor.
pub struct SpeechLike {
    rng: u64,
    t: u64,
    phase: f32,
    f1: Reso,
    f2: Reso,
}

impl SpeechLike {
    pub fn new(seed: u64) -> Self {
        let sr = dsp::SAMPLE_RATE as f32;
        Self {
            rng: seed.max(1),
            t: 0,
            phase: 0.0,
            f1: Reso::new(600.0, 80.0, sr),
            f2: Reso::new(1700.0, 120.0, sr),
        }
    }

    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }

    pub fn fill(&mut self, buf: &mut [f32]) {
        let sr = dsp::SAMPLE_RATE as f32;
        for x in buf.iter_mut() {
            let ts = self.t as f32 / sr;
            let f0 = 120.0 + 20.0 * (ts * 0.7).sin();
            self.phase += f0 / sr;
            let pulse = if self.phase >= 1.0 {
                self.phase -= 1.0;
                1.0
            } else {
                0.0
            };
            let syll = (0.5 - 0.5 * (std::f32::consts::TAU * 4.0 * ts).cos()).powi(2);
            let sentence = if (ts % 5.0) < 3.8 { 1.0 } else { 0.0 };
            let level = 0.25 * (1.0 + 0.5 * (ts * 0.23).sin());
            let src = pulse + 0.02 * self.noise();
            let v = self.f1.tick(src) + 0.6 * self.f2.tick(src);
            *x = level * syll * sentence * v + 0.001 * self.noise();
            self.t += 1;
        }
    }
}

struct Reso {
    a1: f32,
    a2: f32,
    g: f32,
    y1: f32,
    y2: f32,
}

impl Reso {
    fn new(freq: f32, bw: f32, sr: f32) -> Self {
        let r = (-std::f32::consts::PI * bw / sr).exp();
        let th = std::f32::consts::TAU * freq / sr;
        Self {
            a1: 2.0 * r * th.cos(),
            a2: -r * r,
            g: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn tick(&mut self, x: f32) -> f32 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}
