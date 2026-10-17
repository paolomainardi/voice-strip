//! The device timeline, as pure functions.
//!
//! The device runs on the host clock at exactly the nominal rate, on a
//! timeline fixed to host time zero: position `p` is host time
//! `p * ticks_per_frame`. Zero time stamps fall every
//! [`ZERO_TIME_STAMP_PERIOD`] frames on that timeline, so the HAL sees a
//! perfectly steady clock (kAudioDeviceClockAlgorithmRaw) that never
//! re-anchors, and the app computes any timeline position from host time
//! alone, with nothing to read back from the driver.

use crate::model::{SAMPLE_RATE, ZERO_TIME_STAMP_PERIOD};

/// Host ticks per frame from the host's ticks per second (500 on Apple
/// silicon's 24 MHz clock).
pub fn ticks_per_frame(host_ticks_per_second: f64) -> f64 {
    host_ticks_per_second / SAMPLE_RATE
}

/// The latest zero time stamp at or before host time `now`:
/// (sample time, host time).
pub fn zero_time_stamp(now: u64, ticks_per_frame: f64) -> (f64, u64) {
    let tpp = ticks_per_frame * f64::from(ZERO_TIME_STAMP_PERIOD);
    let periods = (now as f64 / tpp).floor();
    (
        periods * f64::from(ZERO_TIME_STAMP_PERIOD),
        (periods * tpp).round() as u64,
    )
}

/// The timeline position of host time `host`.
pub fn position(host: u64, ticks_per_frame: f64) -> i64 {
    (host as f64 / ticks_per_frame).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apple silicon: 24 MHz host clock.
    const TPF: f64 = 500.0;
    const TPP: u64 = 500 * 16_384;

    #[test]
    fn zero_time_stamps_fall_on_whole_periods_of_host_time() {
        assert_eq!(ticks_per_frame(24_000_000.0), TPF);
        assert_eq!(zero_time_stamp(0, TPF), (0.0, 0));
        assert_eq!(zero_time_stamp(TPP - 1, TPF), (0.0, 0));
        assert_eq!(zero_time_stamp(TPP, TPF), (16_384.0, TPP));
        let late = 1_000_000 * TPP + 5;
        assert_eq!(
            zero_time_stamp(late, TPF),
            (1_000_000.0 * 16_384.0, 1_000_000 * TPP)
        );
    }

    #[test]
    fn positions_agree_with_zero_time_stamps() {
        let now = 123_456_789_012;
        let (sample, host) = zero_time_stamp(now, TPF);
        assert_eq!(position(host, TPF), sample as i64);
        assert!(position(now, TPF) >= sample as i64);
        assert!(position(now, TPF) < sample as i64 + 16_384);
    }
}
