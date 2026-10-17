//! Intensity knob, in the design's 120 x 120 coordinate space.

use crate::painter::{LineCap, Painter, Stroke};
use crate::theme::standard as th;

pub const SIZE: f32 = 120.0;
const CX: f32 = 60.0;
const CY: f32 = 60.0;

/// Pointer angle in degrees for an intensity 0..=100: -135 at 0, +135 at 100.
pub fn angle_for(intensity: u8) -> f32 {
    -135.0 + intensity.min(100) as f32 * 2.7
}

/// Drag math: horizontal drag (design uses `cursor: ew-resize`) with
/// vertical drag as an alternative; `pixels_per_unit` logical pixels move
/// the value by one.
pub fn drag_value(start_value: u8, dx: f32, dy: f32, pixels_per_unit: f32) -> u8 {
    let delta = (dx - dy) / pixels_per_unit;
    (start_value as f32 + delta).round().clamp(0.0, 100.0) as u8
}

/// Scroll: one unit per notch, away from the user increases.
pub fn scroll_value(value: u8, notches: f32) -> u8 {
    (value as f32 - notches.round()).clamp(0.0, 100.0) as u8
}

/// Static part: tick marks, body. A decade scale: a mark every 10 units,
/// long at 0, 50 and 100, so the pointer always lands on or between marks
/// that mean a round value.
pub fn paint_body(p: &mut dyn Painter) {
    let major = Stroke::new(th::INK, 1.5).cap(LineCap::Round);
    let minor = Stroke::new(th::INK, 1.0).cap(LineCap::Round);
    for v in (0..=100u8).step_by(10) {
        let a = angle_for(v).to_radians();
        let (sx, cy) = (a.sin(), a.cos());
        let (inner, stroke) = if v % 50 == 0 {
            (46.0, major)
        } else {
            (50.0, minor)
        };
        p.line(
            CX + 54.0 * sx,
            CY - 54.0 * cy,
            CX + inner * sx,
            CY - inner * cy,
            stroke,
        );
    }
    p.fill_circle(CX, CY, 40.0, th::KNOB_OUTER);
    p.fill_circle(CX, CY, 34.0, th::KNOB_INNER);
    p.begin_path();
    p.circle(CX, CY, 34.0);
    p.stroke(Stroke::new(th::KNOB_RING, 2.0));
}

/// Dynamic part: the pointer.
pub fn paint_pointer(p: &mut dyn Painter, intensity: u8) {
    p.save();
    p.translate(CX, CY);
    p.rotate(angle_for(intensity).to_radians());
    p.line(
        0.0,
        -30.0,
        0.0,
        -16.0,
        Stroke::new(th::AMBER, 3.0).cap(LineCap::Round),
    );
    p.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_mapping() {
        assert_eq!(angle_for(0), -135.0);
        assert_eq!(angle_for(50), 0.0);
        assert_eq!(angle_for(100), 135.0);
        assert!((angle_for(60) - 27.0).abs() < 1e-5);
    }

    #[test]
    fn drag_clamps_and_rounds() {
        assert_eq!(drag_value(60, 20.0, 0.0, 2.0), 70);
        assert_eq!(drag_value(60, 0.0, 20.0, 2.0), 50);
        assert_eq!(drag_value(95, 100.0, 0.0, 2.0), 100);
        assert_eq!(drag_value(5, -100.0, 0.0, 2.0), 0);
        assert_eq!(drag_value(60, 1.0, 0.0, 2.0), 61);
    }

    #[test]
    fn scroll_steps() {
        assert_eq!(scroll_value(60, -1.0), 61);
        assert_eq!(scroll_value(0, 1.0), 0);
        assert_eq!(scroll_value(100, -3.0), 100);
    }
}
