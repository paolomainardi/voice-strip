//! The converter runs in the microphone's IOProc: it must not allocate,
//! whatever the block size or ratio changes.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use vdev_macos::convert::Converter;

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

#[test]
fn conversion_does_not_allocate() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let mut c = Converter::new(rate, 1024);
        let mut out = vec![0.0f32; c.max_output()];
        let input = vec![0.25f32; 1024];
        let mut t = 0.0;
        for (i, n) in [64usize, 63, 65, 128, 1, 1024, 512, 64]
            .iter()
            .cycle()
            .take(400)
            .enumerate()
        {
            let position = 10_000.0 + t * 48_000.0 / rate + (i % 3) as f64 * 0.3;
            assert_no_alloc(|| {
                c.process(&input[..*n], t, position, &mut out);
            });
            t += *n as f64;
        }
    }
}
