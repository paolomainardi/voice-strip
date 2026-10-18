//! DeepFilterNet3 (low-latency model) running on a worker thread, exposed to
//! the DSP chain as a [`dsp::FrameDenoiser`].
//!
//! tract allocates during inference, so the model never runs on the audio
//! thread. The audio thread only copies frames into and out of lock-free
//! ring buffers and wakes the worker with `Thread::unpark` (an atomic swap
//! plus a non-blocking futex wake when the worker is parked). The worker runs
//! the model (about 0.15 ms per 10 ms frame on a recent laptop), queues the
//! result, and parks again.
//!
//! The model file is found or downloaded by [`model::locate`].

pub mod model;
pub mod rt;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

/// Safety net if a wakeup is ever lost: the worker re-checks its queue.
const PARK_TIMEOUT: Duration = Duration::from_millis(5);
/// Frames each queue holds: the deepest handoff (the engine's largest host
/// block, 8192 samples, is 18 frames of 480) plus two of slack. About 38 KB
/// per queue.
pub const QUEUE_FRAMES: usize = 20;

#[derive(Debug)]
pub struct InitError(pub String);

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "neural denoiser: {}", self.0)
    }
}

impl std::error::Error for InitError {}

struct Shared {
    stop: AtomicBool,
    atten_lim_db: AtomicU32,
    /// Average run time per frame, ms, f32 bits; NaN before the first.
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

/// Audio-thread side of the neural denoiser.
pub struct DfnDenoiser {
    to_worker: rtrb::Producer<f32>,
    from_worker: rtrb::Consumer<f32>,
    frame: usize,
    model_latency: usize,
    shared: Arc<Shared>,
    priority: rt::Priority,
    /// The worker thread, unparked on every submitted frame.
    wake: std::thread::Thread,
    _worker: Worker,
}

type AudioSide = (
    rtrb::Producer<f32>,
    rtrb::Consumer<f32>,
    usize,
    usize,
    rt::Priority,
);

impl DfnDenoiser {
    /// Loads the model (see [`model::locate`]; it is never downloaded
    /// here) and starts the worker thread. Blocks until the model is ready.
    pub fn spawn() -> Result<Self, InitError> {
        let path = model::locate().map_err(InitError)?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            atten_lim_db: AtomicU32::new(100f32.to_bits()),
            inference_ms: AtomicU32::new(f32::NAN.to_bits()),
        });
        let (tx, rx) = std::sync::mpsc::channel::<Result<AudioSide, String>>();
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dfn-worker".into())
            .spawn(move || {
                let params = RuntimeParams::default_with_ch(1);
                let loaded = DfParams::new(path).and_then(|p| DfTract::new(p, &params));
                let mut model = match loaded {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let hop = model.hop_size;
                let latency = model.fft_size - hop + model.lookahead * hop;
                // The error fallback in `run` delays by one hop.
                debug_assert_eq!(latency, hop, "fallback assumes one hop of model delay");
                let (to_worker, worker_in) = rtrb::RingBuffer::new(hop * QUEUE_FRAMES);
                let (worker_out, from_worker) = rtrb::RingBuffer::new(hop * QUEUE_FRAMES);
                // Only after the model is loaded: loading is long CPU work
                // without blocking and would exceed RLIMIT_RTTIME.
                let priority = rt::raise();
                if tx
                    .send(Ok((to_worker, from_worker, hop, latency, priority)))
                    .is_err()
                {
                    return;
                }
                run(&mut model, hop, worker_in, worker_out, &worker_shared);
            })
            .map_err(|e| InitError(e.to_string()))?;
        let wake = thread.thread().clone();
        let worker = Worker {
            shared: shared.clone(),
            thread: Some(thread),
        };
        let (to_worker, from_worker, frame, model_latency, priority) = rx
            .recv()
            .map_err(|_| InitError("worker exited during setup".into()))?
            .map_err(InitError)?;
        Ok(Self {
            to_worker,
            from_worker,
            frame,
            model_latency,
            shared,
            priority,
            wake,
            _worker: worker,
        })
    }

    /// How the worker thread got scheduled.
    pub fn priority(&self) -> rt::Priority {
        self.priority
    }
}

fn run(
    model: &mut DfTract,
    hop: usize,
    mut input: rtrb::Consumer<f32>,
    mut output: rtrb::Producer<f32>,
    shared: &Shared,
) {
    let mut noisy = Array2::<f32>::zeros((1, hop));
    let mut enh = Array2::<f32>::zeros((1, hop));
    let mut prev = Array2::<f32>::zeros((1, hop));
    let mut atten = f32::NAN;
    let mut avg_ms = f32::NAN;
    while !shared.stop.load(Ordering::Relaxed) {
        if input.slots() < hop {
            std::thread::park_timeout(PARK_TIMEOUT);
            continue;
        }
        let a = f32::from_bits(shared.atten_lim_db.load(Ordering::Relaxed));
        if a != atten {
            atten = a;
            model.set_atten_lim(a);
        }
        if let Ok(chunk) = input.read_chunk(hop) {
            let (a, b) = chunk.as_slices();
            for (d, s) in noisy.iter_mut().zip(a.iter().chain(b)) {
                *d = *s;
            }
            chunk.commit_all();
        }
        // On a model error deliver the input delayed by the model latency
        // (exactly one hop for this model), so the fallback stays aligned
        // with the dry path instead of muting or doubling the voice.
        let t = Instant::now();
        if model.process(noisy.view(), enh.view_mut()).is_err() {
            enh.assign(&prev);
        }
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        avg_ms = if avg_ms.is_nan() {
            ms
        } else {
            avg_ms + (ms - avg_ms) * 0.02
        };
        shared
            .inference_ms
            .store(avg_ms.to_bits(), Ordering::Relaxed);
        prev.assign(&noisy);
        // Never drop a result: if the audio side has not fetched yet, wait
        // for room (dropping would desynchronise the frame count).
        while output.slots() < hop && !shared.stop.load(Ordering::Relaxed) {
            std::thread::park_timeout(PARK_TIMEOUT);
        }
        if let Ok(mut chunk) = output.write_chunk_uninit(hop) {
            let (a, b) = chunk.as_mut_slices();
            for (d, s) in a.iter_mut().chain(b.iter_mut()).zip(enh.iter()) {
                d.write(*s);
            }
            // SAFETY: all `hop` slots were written above.
            unsafe { chunk.commit_all() };
        }
    }
}

impl dsp::FrameDenoiser for DfnDenoiser {
    fn frame_len(&self) -> usize {
        self.frame
    }

    fn model_latency_samples(&self) -> usize {
        self.model_latency
    }

    fn submit(&mut self, frame: &[f32]) -> bool {
        let Ok(mut chunk) = self.to_worker.write_chunk_uninit(frame.len()) else {
            return false;
        };
        let (a, b) = chunk.as_mut_slices();
        for (d, s) in a.iter_mut().chain(b.iter_mut()).zip(frame) {
            d.write(*s);
        }
        // SAFETY: all slots were written above.
        unsafe { chunk.commit_all() };
        self.wake.unpark();
        true
    }

    fn fetch(&mut self, out: &mut [f32]) -> bool {
        let Ok(chunk) = self.from_worker.read_chunk(out.len()) else {
            return false;
        };
        let (a, b) = chunk.as_slices();
        for (d, s) in out.iter_mut().zip(a.iter().chain(b)) {
            *d = *s;
        }
        chunk.commit_all();
        true
    }

    fn max_in_flight(&self) -> usize {
        QUEUE_FRAMES
    }

    fn inference_ms(&self) -> Option<f32> {
        let v = f32::from_bits(self.shared.inference_ms.load(Ordering::Relaxed));
        (!v.is_nan()).then_some(v)
    }

    fn set_attenuation_limit_db(&mut self, db: f32) {
        self.shared
            .atten_lim_db
            .store(db.to_bits(), Ordering::Relaxed);
    }
}
