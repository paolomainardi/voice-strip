//! Voice activity: whether someone is speaking, from a speech detector
//! running outside the audio thread (Silero VAD, provided by the `vad`
//! crate through [`VoiceDetector`]).
//!
//! The chain never gates audio on it. It only steers measurements: the
//! expander learns the room noise while nobody speaks, and the leveler
//! measures loudness only around speech.

/// A speech detector running outside the audio thread. `push` and
/// `speech_probability` are called on the audio thread and must be
/// wait-free and allocation-free.
pub trait VoiceDetector: Send {
    /// Queues input samples at 48 kHz. Samples that do not fit are dropped.
    fn push(&mut self, block: &[f32]);
    /// Latest speech probability, 0 to 1, `None` before the first result.
    fn speech_probability(&self) -> Option<f32>;
    /// Average model run time per analysed chunk, ms, `None` before the
    /// first result.
    fn inference_ms(&self) -> Option<f32> {
        None
    }
}

/// Probability at which speech starts.
const SPEECH_ON: f32 = 0.5;
/// Probability below which speech ends (Silero's own hysteresis).
const SPEECH_OFF: f32 = 0.35;

/// Speech or not from a stream of probabilities, with hysteresis.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpeechGate {
    speech: bool,
}

impl SpeechGate {
    pub fn update(&mut self, p: f32) -> bool {
        if p >= SPEECH_ON {
            self.speech = true;
        } else if p < SPEECH_OFF {
            self.speech = false;
        }
        self.speech
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_holds_between_thresholds() {
        let mut g = SpeechGate::default();
        assert!(!g.update(0.4));
        assert!(g.update(0.6));
        assert!(g.update(0.4));
        assert!(!g.update(0.2));
    }
}
