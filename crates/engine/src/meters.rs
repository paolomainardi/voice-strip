/// Meter snapshot sent from the audio thread to the UI every 480 samples
/// (10 ms). `Copy`, fixed size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterFrame {
    /// Input sample peak over the block, dBFS.
    pub in_peak_db: f32,
    /// Input RMS over the block, dBFS.
    pub in_rms_db: f32,
    /// Output RMS over the block, dBFS.
    pub out_rms_db: f32,
    /// Input average-rectified level over the block, dBFS, scaled so that
    /// a sine reads its RMS (drives the input VU, which reads the average
    /// like a real VU rectifier).
    pub in_avg_db: f32,
    /// Output average-rectified level, dBFS (drives the output VU).
    pub out_avg_db: f32,
    /// Momentary loudness (400 ms), LUFS.
    pub lufs_momentary: f32,
    /// Short-term loudness (3 s), LUFS.
    pub lufs_short_term: f32,
    /// Output true peak over the block, dBTP.
    pub true_peak_db: f32,
    /// Gain reduction per module, dB (positive), indexed by `dsp::Module::index`.
    pub gain_reduction_db: [f32; 9],
    /// Frames the neural denoiser of the main chain failed to deliver in
    /// time since that chain started (cumulative).
    pub denoise_missed_frames: u64,
}

/// Average of |x| over `buf`, scaled by pi / (2 sqrt 2) so that a sine
/// reads its RMS: the full-wave average rectifier of a VU meter. Speech,
/// with its high crest factor, reads lower than its RMS, as on a real VU.
pub fn rectified_average(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    const SINE_RMS_OVER_AVERAGE: f32 = std::f32::consts::PI / (2.0 * std::f32::consts::SQRT_2);
    buf.iter().map(|x| x.abs()).sum::<f32>() / buf.len() as f32 * SINE_RMS_OVER_AVERAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rms_dbfs: f32, n: usize) -> Vec<f32> {
        let amp = 10f32.powf(rms_dbfs / 20.0) * std::f32::consts::SQRT_2;
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn a_sine_reads_its_rms() {
        // 1 kHz in a 10 ms block: ten whole cycles.
        let block = sine(1000.0, -18.0, 480);
        let db = 20.0 * rectified_average(&block).log10();
        assert!((db - (-18.0)).abs() < 0.1, "{db} dBFS");
    }

    #[test]
    fn speech_like_signals_read_below_rms() {
        // A pulse train: high crest factor, like plosive-rich speech.
        let block: Vec<f32> = (0..480)
            .map(|i| if i % 48 < 4 { 0.5 } else { 0.0 })
            .collect();
        let rms = (block.iter().map(|x| x * x).sum::<f32>() / 480.0).sqrt();
        assert!(rectified_average(&block) < rms);
        assert_eq!(rectified_average(&[]), 0.0);
    }
}
