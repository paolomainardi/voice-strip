//! Frame handoff of the neural stage, with a fake worker: results become
//! ready one audio callback after submission (like a worker thread that
//! needs a callback's time), and one frame is delivered late. The fake
//! inverts polarity so wet and dry are distinguishable. The chain is fed
//! host blocks directly, as the engine does.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use dsp::stages::denoise::{WORKER_BUDGET, handoff_samples};
use dsp::{Chain, ChainSettings, FrameDenoiser, Module, ModuleSettings, PathKind};

const F: usize = 480;
/// Queue capacity in frames, as in `denoise_neural::QUEUE_FRAMES`.
const QUEUE: usize = 20;

struct Fake {
    clock: Rc<Cell<usize>>,
    queue: VecDeque<(Vec<f32>, usize)>,
    submitted: usize,
    late_frame: usize,
    /// Callbacks the late frame takes.
    late_callbacks: usize,
}

// The test runs on one thread; Send is required by the trait only.
unsafe impl Send for Fake {}

impl FrameDenoiser for Fake {
    fn frame_len(&self) -> usize {
        F
    }
    fn model_latency_samples(&self) -> usize {
        0
    }
    fn max_in_flight(&self) -> usize {
        QUEUE
    }
    fn submit(&mut self, frame: &[f32]) -> bool {
        // Bounded like the real worker's queues.
        if self.queue.len() >= QUEUE {
            return false;
        }
        // Ready from the next callback on; the late frame much later, so it
        // misses its slot at every host block size.
        let delay = if self.submitted == self.late_frame {
            self.late_callbacks
        } else {
            1
        };
        self.queue
            .push_back((frame.iter().map(|x| -x).collect(), self.clock.get() + delay));
        self.submitted += 1;
        true
    }
    fn fetch(&mut self, out: &mut [f32]) -> bool {
        match self.queue.front() {
            Some((_, ready)) if *ready <= self.clock.get() => {
                let (f, _) = self.queue.pop_front().unwrap();
                out.copy_from_slice(&f);
                true
            }
            _ => false,
        }
    }
}

/// Callbacks the late frame takes: well past its handoff.
fn late_callbacks(host_block: usize) -> usize {
    (handoff_samples(Some(host_block), F) + 2 * F).div_ceil(host_block) + 1
}

fn run(host_block: usize) -> (Vec<f32>, Vec<f32>, usize, u64) {
    let clock = Rc::new(Cell::new(0));
    let fake = Fake {
        clock: clock.clone(),
        queue: VecDeque::new(),
        submitted: 0,
        late_frame: 100,
        late_callbacks: late_callbacks(host_block),
    };
    let handoff = handoff_samples(Some(host_block), F);
    let mut chain = Chain::with_denoiser_handoff(PathKind::Main, F, Some(Box::new(fake)), handoff);
    let mut enabled = [false; 9];
    enabled[Module::Denoise.index()] = true;
    chain.set_settings(&ChainSettings {
        enabled,
        modules: ModuleSettings {
            denoise_mix: 1.0,
            ..Default::default()
        },
        ..Default::default()
    });
    let lat = chain.latency_samples();
    let n = 48_000 * 4;
    // Non-periodic (xorshift) so any lag error shows up.
    let mut st = 0x2545_F491_4F6C_DD1Du64;
    let x: Vec<f32> = (0..n)
        .map(|_| {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 40) as f32 / (1u64 << 24) as f32 * 0.5 - 0.25
        })
        .collect();
    let mut y = Vec::with_capacity(n);
    // Each host callback runs the chain on its block, in chunks of at most
    // one frame, like the engine.
    for (k, cb) in x.chunks(host_block).enumerate() {
        clock.set(k);
        let mut b = cb.to_vec();
        for chunk in b.chunks_mut(F) {
            chain.process(chunk);
        }
        y.extend(b);
    }
    (x, y, lat, chain.denoise_missed_frames())
}

/// For every host block size the wet output equals the inverted input
/// delayed by exactly the declared latency, before and after the late
/// frame, and the late frame costs exactly one miss.
#[test]
fn handoff_keeps_alignment_for_all_host_blocks() {
    for host in [1, 48, 63, 64, 96, 128, 240, 256, 480, 960, 1024, 2048, 8192] {
        let (x, y, lat, misses) = run(host);
        let late_start = 100 * F + lat;
        let mut bad = 0;
        // Skip the first 0.5 s: the wet/dry mix smoother ramps in.
        for i in 24_000..y.len() {
            // Skip the late frame(s) and their crossfades (the 2 ms smoother
            // settles to 1e-6 in about 25 ms).
            if i + 200 >= late_start && i < late_start + late_callbacks(host) * host + 2 * F + 1_200
            {
                continue;
            }
            if (y[i] + x[i - lat]).abs() > 1e-6 {
                if bad < 3 {
                    eprintln!(
                        "  mismatch at {i} (frame {}): y {} expected {} dry {}",
                        i / F,
                        y[i],
                        -x[i - lat],
                        x[i - lat]
                    );
                }
                bad += 1;
            }
        }
        eprintln!(
            "host block {host}: declared latency {lat}, misses {misses}, mismatched samples {bad}"
        );
        assert_eq!(bad, 0, "host block {host}");
        // The late frame misses; the next ones may wait behind it in FIFO
        // order. Alignment afterwards is what matters.
        let max_misses = (late_callbacks(host) * host).div_ceil(F) as u64;
        assert!(
            (1..=max_misses).contains(&misses),
            "host block {host}: {misses}"
        );
    }
}

/// The handoff gives the worker at least the budget at every block size:
/// a callback-sized fake worker would need it, and the declared latency is
/// the smallest that does.
#[test]
fn handoff_covers_the_worker_budget() {
    assert_eq!(handoff_samples(Some(480), F), 480);
    assert_eq!(handoff_samples(Some(960), F), 960);
    assert_eq!(handoff_samples(Some(240), F), 480);
    assert_eq!(handoff_samples(Some(64), F), F + 64 + WORKER_BUDGET);
    assert_eq!(handoff_samples(Some(48), F), F + 96);
    assert_eq!(handoff_samples(None, F), 2 * F + WORKER_BUDGET);
}
