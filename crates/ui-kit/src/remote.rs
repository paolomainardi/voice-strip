//! Meters for remote clients (desktop bars, a macOS menu bar extra): the
//! standard view's ballistics, so a bar's VU needles move exactly like the
//! window's.

use app_core::UiState;
use app_core::remote::DisplayMeters;

use crate::ballistics::{GrSmoother, PeakHold, Smoother, VuBallistics};
use crate::standard::{PEAK_LAMP_DBFS, StandardView, vu_inputs_db};

pub struct RemoteMeters {
    vu_in: VuBallistics,
    vu_out: VuBallistics,
    peak: PeakHold,
    gr: GrSmoother,
    lufs: Smoother,
    lufs_valid: bool,
}

impl RemoteMeters {
    pub fn new() -> Self {
        Self {
            vu_in: VuBallistics::new(),
            vu_out: VuBallistics::new(),
            peak: PeakHold::new(PEAK_LAMP_DBFS, 0.5),
            gr: GrSmoother::default(),
            lufs: Smoother::new(-24.0, 0.4),
            lufs_valid: false,
        }
    }

    /// Advances by `dt` seconds with the latest meters in `state`.
    pub fn update(&mut self, state: &UiState, dt: f32) -> DisplayMeters {
        let m = &state.meters;
        let (in_level_db, out_level_db) = vu_inputs_db(m);
        let in_vu_db = self.vu_in.update(in_level_db, dt);
        let out_vu_db = self.vu_out.update(out_level_db, dt);
        let peak = self.peak.update(m.in_peak_db, dt);
        let gr_db = self.gr.update(StandardView::total_gr_db(state), dt);
        // No loudness below the -70 LUFS gate (silence, muted), as in the
        // window: the reading goes away instead of freezing.
        self.lufs_valid = m.lufs_short_term.is_finite();
        if self.lufs_valid {
            self.lufs.update(m.lufs_short_term, dt);
        }
        DisplayMeters {
            in_vu_db,
            out_vu_db,
            in_level_db,
            out_level_db,
            gr_db,
            lufs: self.lufs_valid.then(|| self.lufs.value()),
            peak,
        }
    }
}

impl Default for RemoteMeters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_input_like_the_window() {
        let (handle, _processor) = engine::new_engine();
        let mut state = app_core::Controller::new(handle).state().clone();
        state.meters.in_avg_db = f32::NEG_INFINITY;
        state.meters.out_avg_db = f32::NEG_INFINITY;
        state.meters.in_peak_db = f32::NEG_INFINITY;
        state.meters.lufs_short_term = f32::NEG_INFINITY;
        let mut m = RemoteMeters::new();
        let first = m.update(&state, 1.0 / 30.0);
        assert!(first.in_vu_db <= -60.0, "{first:?}");
        assert_eq!(first.lufs, None);
        assert!(!first.peak);
        state.meters.in_avg_db = crate::standard::VU_REFERENCE_DBFS;
        state.meters.in_peak_db = 0.0;
        state.meters.lufs_short_term = -16.0;
        let mut last = first;
        for _ in 0..30 {
            last = m.update(&state, 1.0 / 30.0);
        }
        // One second at 0 VU: the needle sits at 0.
        assert!(last.in_vu_db.abs() < 0.2, "{last:?}");
        assert!(last.in_level_db.abs() < 1e-4, "{last:?}");
        assert!(last.peak);
        assert!(last.lufs.is_some());
        // Silence again: the loudness reading goes away.
        state.meters.lufs_short_term = f32::NEG_INFINITY;
        assert_eq!(m.update(&state, 1.0 / 30.0).lufs, None);
    }
}
