//! Standby policy, run by each backend on its own non-realtime thread so it
//! keeps working when the window draws nothing (a window on a hidden
//! workspace gets no frames): capture pauses [`STANDBY_AFTER`] after the
//! last application stopped reading the virtual microphone, unless the
//! monitor or a calibration holds it, and resumes as soon as one of them
//! needs it.

use std::time::{Duration, Instant};

/// How long the microphone stays open after the last application stopped
/// reading Voice Strip.
pub const STANDBY_AFTER: Duration = Duration::from_secs(5);

pub struct Policy {
    needed_at: Instant,
    standby: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self::new()
    }
}

impl Policy {
    pub fn new() -> Self {
        Self {
            needed_at: Instant::now(),
            standby: false,
        }
    }

    /// `readers`: whether an application reads the virtual microphone,
    /// `None` when unknown (counts as needed); `held`: monitor or
    /// calibration. Returns the new state when it changes: `Some(true)` to
    /// pause capture, `Some(false)` to resume it.
    pub fn update(&mut self, readers: Option<bool>, held: bool) -> Option<bool> {
        let needed = readers != Some(false) || held;
        if needed {
            self.needed_at = Instant::now();
        }
        let standby = !needed && self.needed_at.elapsed() >= STANDBY_AFTER;
        (standby != self.standby).then(|| {
            self.standby = standby;
            standby
        })
    }

    /// A change could not be applied (a failed resume): try again next time.
    pub fn revert(&mut self) {
        self.standby = !self.standby;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_after_the_grace_and_resumes_at_once() {
        let mut p = Policy::new();
        assert_eq!(p.update(Some(true), false), None);
        assert_eq!(p.update(Some(false), false), None, "grace period");
        p.needed_at -= STANDBY_AFTER;
        assert_eq!(p.update(Some(false), false), Some(true));
        assert_eq!(p.update(Some(false), false), None);
        assert_eq!(p.update(Some(false), true), Some(false), "held");
        p.needed_at -= STANDBY_AFTER;
        assert_eq!(
            p.update(None, false),
            None,
            "unknown readers count as needed"
        );
        assert_eq!(p.update(Some(true), false), None);
    }
}
