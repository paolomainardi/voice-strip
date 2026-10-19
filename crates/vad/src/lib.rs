//! Silero VAD (voice activity detection) running on a worker thread,
//! exposed to the engine as a [`dsp::VoiceDetector`].
//!
//! The model is small (2.8 MB) and embedded in the binary, so speech
//! detection is always available, with no download. `build.rs` fetches it
//! at a pinned commit and checks its SHA-256.
//!
//! The audio thread only copies samples into a lock-free ring and wakes the
//! worker. The worker low-passes and decimates 48 kHz to the model's
//! 16 kHz, runs the model on 512-sample chunks (32 ms) with 64 samples of
//! context and the recurrent state carried over, and publishes the speech
//! probability and the run time as atomics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dsp::util::{Biquad, Coefs};
use tract_onnx::prelude::*;

static MODEL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/silero_vad.onnx"));

/// Model name and version, for display.
pub const MODEL_NAME: &str = "Silero VAD";
pub const MODEL_VERSION: &str = "6.2.3";

/// Model rate and chunk.
const RATE: i64 = 16_000;
const DECIMATION: usize = 3;
const CHUNK: usize = 512;
const CONTEXT: usize = 64;
/// One second of input at 48 kHz.
const QUEUE: usize = 48_000;
/// Safety net if a wakeup is ever lost.
const PARK_TIMEOUT: Duration = Duration::from_millis(10);
/// Run time average per chunk.
const TIME_AVG: f32 = 0.05;

#[derive(Debug)]
pub struct InitError(pub String);

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "voice detector: {}", self.0)
    }
}

impl std::error::Error for InitError {}

struct Shared {
    stop: AtomicBool,
    /// Speech probability, f32 bits; NaN before the first result.
    probability: AtomicU32,
    /// Average run time per chunk, ms, f32 bits; NaN before the first.
    inference_ms: AtomicU32,
}

struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

type Plan = TypedRunnableModel;

/// Audio-thread side of the voice detector.
pub struct SileroVad {
    to_worker: rtrb::Producer<f32>,
    /// Samples pushed since the worker was last woken.
    unwoken: usize,
    shared: Arc<Shared>,
    wake: std::thread::Thread,
    _worker: Worker,
}

impl SileroVad {
    /// Loads the embedded model and starts the worker thread. Blocks until
    /// the model is ready.
    pub fn spawn() -> Result<Self, InitError> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            probability: AtomicU32::new(f32::NAN.to_bits()),
            inference_ms: AtomicU32::new(f32::NAN.to_bits()),
        });
        let (to_worker, from_audio) = rtrb::RingBuffer::new(QUEUE);
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vad-worker".into())
            .spawn(move || {
                let model = match load() {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if tx.send(Ok(())).is_err() {
                    return;
                }
                run(&model.0, &model.1, from_audio, &worker_shared);
            })
            .map_err(|e| InitError(e.to_string()))?;
        let wake = thread.thread().clone();
        let worker = Worker {
            shared: shared.clone(),
            thread: Some(thread),
        };
        rx.recv()
            .map_err(|_| InitError("worker exited during setup".into()))?
            .map_err(InitError)?;
        Ok(Self {
            to_worker,
            unwoken: 0,
            shared,
            wake,
            _worker: worker,
        })
    }
}

/// The optimized model and the order of its inputs.
fn load() -> TractResult<(Arc<Plan>, Vec<String>)> {
    let mut model = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .with_ignore_value_info(true)
        .model_for_read(&mut &MODEL[..])?;
    let names: Vec<String> = model
        .input_outlets()?
        .iter()
        .map(|o| model.node(o.node).name.clone())
        .collect();
    for (i, n) in names.iter().enumerate() {
        let fact: InferenceFact = match n.as_str() {
            "input" => f32::fact([1, CONTEXT + CHUNK]).into(),
            "state" => f32::fact([2, 1, 128]).into(),
            // A constant rate lets tract fold the rate-dependent branches.
            "sr" => InferenceFact::from(tensor0(RATE)),
            other => return Err(TractError::msg(format!("unexpected model input {other}"))),
        };
        model.set_input_fact(i, fact)?;
    }
    Ok((model.into_optimized()?.into_runnable()?, names))
}

/// One chunk: speech probability and the next state.
fn infer(
    plan: &Arc<Plan>,
    names: &[String],
    input: &[f32],
    state: &Tensor,
) -> TractResult<(f32, Tensor)> {
    let mut inputs = TVec::new();
    for n in names {
        inputs.push(match n.as_str() {
            "input" => Tensor::from_shape(&[1, CONTEXT + CHUNK], input)?.into(),
            "state" => state.clone().into(),
            _ => tensor0(RATE).into(),
        });
    }
    let out = plan.run(inputs)?;
    let p = out[0]
        .try_as_plain_ram()?
        .as_slice::<f32>()?
        .first()
        .copied()
        .unwrap_or(0.0);
    Ok((p, out[1].clone().into_tensor()))
}

fn run(plan: &Arc<Plan>, names: &[String], mut input: rtrb::Consumer<f32>, shared: &Shared) {
    // Fourth-order Butterworth at 7 kHz before keeping one sample in three.
    let mut lp = [
        Biquad::new(Coefs::lowpass(7_000.0, 0.541)),
        Biquad::new(Coefs::lowpass(7_000.0, 1.307)),
    ];
    let mut phase = 0;
    // Context followed by the chunk being filled.
    let mut buf = vec![0.0f32; CONTEXT + CHUNK];
    let mut fill = CONTEXT;
    let mut state = Tensor::zero::<f32>(&[2, 1, 128]).expect("state tensor");
    let mut avg_ms = f32::NAN;
    while !shared.stop.load(Ordering::Relaxed) {
        let n = input.slots();
        if n == 0 {
            std::thread::park_timeout(PARK_TIMEOUT);
            continue;
        }
        let Ok(chunk) = input.read_chunk(n) else {
            continue;
        };
        for x in chunk {
            let y = lp[0].tick(x);
            let y = lp[1].tick(y);
            phase += 1;
            if phase < DECIMATION {
                continue;
            }
            phase = 0;
            buf[fill] = y;
            fill += 1;
            if fill < buf.len() {
                continue;
            }
            let t = Instant::now();
            match infer(plan, names, &buf, &state) {
                Ok((p, next)) => {
                    state = next;
                    shared.probability.store(p.to_bits(), Ordering::Relaxed);
                }
                // Start over from silence rather than keep a bad state.
                Err(_) => state = Tensor::zero::<f32>(&[2, 1, 128]).expect("state tensor"),
            }
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            avg_ms = if avg_ms.is_nan() {
                ms
            } else {
                avg_ms + (ms - avg_ms) * TIME_AVG
            };
            shared
                .inference_ms
                .store(avg_ms.to_bits(), Ordering::Relaxed);
            buf.copy_within(CHUNK.., 0);
            fill = CONTEXT;
        }
    }
}

fn load_f32(a: &AtomicU32) -> Option<f32> {
    let v = f32::from_bits(a.load(Ordering::Relaxed));
    (!v.is_nan()).then_some(v)
}

impl dsp::VoiceDetector for SileroVad {
    fn push(&mut self, block: &[f32]) {
        let n = block.len().min(self.to_worker.slots());
        if let Ok(mut chunk) = self.to_worker.write_chunk_uninit(n) {
            let (a, b) = chunk.as_mut_slices();
            for (d, s) in a.iter_mut().chain(b.iter_mut()).zip(block) {
                d.write(*s);
            }
            // SAFETY: all `n` slots were written above.
            unsafe { chunk.commit_all() };
        }
        // Woken once per model chunk, not on every (possibly tiny) host
        // block: each wakeup is a system call on the audio thread.
        self.unwoken += block.len();
        if self.unwoken >= CHUNK * DECIMATION {
            self.unwoken = 0;
            self.wake.unpark();
        }
    }

    fn speech_probability(&self) -> Option<f32> {
        load_f32(&self.shared.probability)
    }

    fn inference_ms(&self) -> Option<f32> {
        load_f32(&self.shared.inference_ms)
    }
}
