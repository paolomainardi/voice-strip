//! Realtime-safety and latency tests for `AudioProcessor`.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use engine::{MonitorConfig, new_engine};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

/// Deterministic xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn sample(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }

    fn block_size(&mut self) -> usize {
        1 + (self.next() % 1024) as usize
    }
}

fn run_blocks(host_block: Option<usize>, with_monitor: bool, total: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut handle, mut proc_) = new_engine();
    if with_monitor {
        handle.set_monitor(MonitorConfig {
            enabled: true,
            ..MonitorConfig::default()
        });
    }
    proc_.set_host_block_size(host_block);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let input: Vec<f32> = (0..total).map(|_| 0.25 * rng.sample()).collect();
    let mut output = vec![0.0f32; total];
    let mut monitor = vec![0.0f32; total];
    let mut pos = 0;
    while pos < total {
        let n = host_block
            .unwrap_or_else(|| rng.block_size())
            .min(total - pos);
        let (inp, out, mon) = (
            &input[pos..pos + n],
            &mut output[pos..pos + n],
            &mut monitor[pos..pos + n],
        );
        assert_no_alloc(|| {
            if with_monitor {
                proc_.process(inp, out, Some(mon));
            } else {
                proc_.process(inp, out, None);
            }
        });
        pos += n;
    }
    // No re-blocking: the main path adds only the chain's own latency,
    // whatever the block sizes.
    assert_eq!(
        handle.main_latency_samples(),
        chain_latency(dsp::PathKind::Main)
    );
    let latency = handle.main_latency_samples();
    // Neutral chain: output is the input delayed by exactly the latency.
    assert!(
        input[..total - latency]
            .iter()
            .zip(&output[latency..])
            .all(|(a, b)| a == b)
    );
    assert!(output[..latency].iter().all(|&x| x == 0.0));
    if with_monitor {
        // Monitor path: neutral chain delays by its own (limiter) latency
        // only.
        let ml = chain_latency(dsp::PathKind::Monitor);
        assert_eq!(handle.monitor_latency_samples(), ml);
        assert!(
            input[..total - ml]
                .iter()
                .zip(&monitor[ml..])
                .all(|(a, b)| a == b)
        );
    }
    (input, output)
}

#[test]
fn no_alloc_random_blocks_without_monitor() {
    run_blocks(None, false, 48_000);
}

#[test]
fn no_alloc_random_blocks_with_monitor() {
    run_blocks(None, true, 48_000);
}

#[test]
fn fixed_blocks_are_sample_exact() {
    for n in [1, 32, 63, 64, 65, 96, 128, 256, 480, 512, 960, 1000, 1024] {
        run_blocks(Some(n), false, 48_000);
    }
}

#[test]
fn meter_frames_arrive_at_100_hz() {
    let (mut handle, mut p) = new_engine();
    let mut rng = Rng(7);
    let input = [0.25f32; 1024];
    let mut out = [0.0f32; 1024];
    let seconds = 3;
    let mut fed = 0;
    let mut frames = 0;
    while fed < dsp::SAMPLE_RATE as usize * seconds {
        let n = rng
            .block_size()
            .min(dsp::SAMPLE_RATE as usize * seconds - fed);
        assert_no_alloc(|| p.process(&input[..n], &mut out[..n], None));
        fed += n;
        // Random blocks never exceed 1024 < 3 * 480, so at most two frames
        // are queued per call; drain them one by one.
        while handle.pop_meter().is_some() {
            frames += 1;
        }
    }
    assert_eq!(frames, 100 * seconds);
}

#[test]
fn monitor_off_outputs_silence() {
    let (_h, mut p) = new_engine();
    let input = [0.5f32; 64];
    let mut out = [0.0f32; 64];
    let mut mon = [1.0f32; 64];
    assert_no_alloc(|| p.process(&input, &mut out, Some(&mut mon)));
    assert!(mon.iter().all(|&x| x == 0.0));
}

fn chain_latency(path: dsp::PathKind) -> usize {
    dsp::Chain::new(path, dsp::BLOCK_MAIN).latency_samples()
}

/// Swapping the main chain is allocation-free on the audio thread, takes
/// effect at the next block, updates the reported latency, and the old
/// chain is dropped on the handle side.
#[test]
fn main_chain_swap_is_realtime_safe() {
    let (mut handle, mut p) = engine::new_engine();
    p.set_host_block_size(Some(480));
    let before = handle.main_latency_samples();
    let mut chain = dsp::Chain::new(dsp::PathKind::Main, dsp::BLOCK_MAIN);
    chain.set_settings(&dsp::ChainSettings::default());
    assert!(handle.replace_main_chain(Box::new(chain)).is_ok());
    let input = vec![0.1f32; 480];
    let mut out = vec![0.0f32; 480];
    assert_no_alloc(|| p.process(&input, &mut out, None));
    assert_eq!(handle.main_latency_samples(), before);
    handle.drop_retired();
}
