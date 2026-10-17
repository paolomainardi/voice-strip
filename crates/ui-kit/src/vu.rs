//! Analog VU meter face and needle, in the design's 200 x 130 coordinate
//! space (`design/Standard.dc.html`).

use crate::painter::{Align, Color, Family, LineCap, Painter, Rect, Stroke, TextStyle};
use crate::theme::standard as th;

pub const VIEWBOX_W: f32 = 200.0;
pub const VIEWBOX_H: f32 = 130.0;

/// Scale arc center and radius.
const ARC_CX: f32 = 100.0;
const ARC_CY: f32 = 110.0;
const ARC_R: f32 = 82.0;
const TICK_R_IN: f32 = 72.0;
/// Needle pivot.
const PIVOT_X: f32 = 100.0;
const PIVOT_Y: f32 = 112.0;
const NEEDLE_TIP_Y: f32 = 34.0;

/// dB to needle angle (degrees, 0 = straight up), piecewise linear.
const ANGLE_TABLE: [(f32, f32); 10] = [
    (-20.0, -50.0),
    (-10.0, -30.0),
    (-7.0, -20.0),
    (-5.0, -12.0),
    (-3.0, -3.0),
    (-1.0, 8.0),
    (0.0, 15.0),
    (1.0, 24.0),
    (2.0, 35.0),
    (3.0, 50.0),
];

/// Needle angle at rest (no signal), degrees.
pub const REST_ANGLE: f32 = -52.0;

/// Needle angle in degrees for a level in dB (VU reference 0 dB). Below
/// the scale's -20 dB the needle keeps moving towards rest in proportion
/// to the linear level, as a moving coil does, so there is no jump.
pub fn angle_for(db: f32) -> f32 {
    if db.is_nan() {
        return REST_ANGLE;
    }
    let (d0, a0) = ANGLE_TABLE[0];
    if db <= d0 {
        let k = 10f32.powf((db - d0) / 20.0);
        return REST_ANGLE + k * (a0 - REST_ANGLE);
    }
    if db >= 3.0 {
        return 50.0;
    }
    for w in ANGLE_TABLE.windows(2) {
        let (d0, a0) = w[0];
        let (d1, a1) = w[1];
        if db >= d0 && db <= d1 {
            let k = (db - d0) / (d1 - d0);
            return a0 + k * (a1 - a0);
        }
    }
    0.0
}

fn polar(r: f32, deg: f32) -> (f32, f32) {
    let a = deg.to_radians();
    (ARC_CX + r * a.sin(), ARC_CY - r * a.cos())
}

/// Static face: bezel, scale arcs, ticks, labels. Caller sets the transform
/// so that the 200 x 130 box maps onto the widget.
pub fn paint_face(p: &mut dyn Painter) {
    p.fill_rounded_rect(Rect::new(1.0, 1.0, 198.0, 128.0), 8.0, th::VU_FACE);
    p.begin_path();
    p.rounded_rect(Rect::new(1.0, 1.0, 198.0, 128.0), 8.0);
    p.stroke(Stroke::new(th::INK, 2.0));

    // Black arc -50..15 degrees, red arc 15..50.
    p.begin_path();
    p.arc(
        ARC_CX,
        ARC_CY,
        ARC_R,
        (-50.0f32 - 90.0).to_radians(),
        (15.0f32 - 90.0).to_radians(),
    );
    p.stroke(Stroke::new(th::INK, 1.5));
    p.begin_path();
    p.arc(
        ARC_CX,
        ARC_CY,
        ARC_R,
        (15.0f32 - 90.0).to_radians(),
        (50.0f32 - 90.0).to_radians(),
    );
    p.stroke(Stroke::new(th::RED, 4.0));

    // Ticks at -20, -10, -7, -5, -3, -1 (ink) and 0, 1, 2, 3 (red).
    for &(db, deg) in ANGLE_TABLE.iter() {
        let color = if db >= 0.0 { th::RED } else { th::INK };
        let (x0, y0) = polar(ARC_R, deg);
        let (x1, y1) = polar(TICK_R_IN, deg);
        p.line(x0, y0, x1, y1, Stroke::new(color, 1.5));
    }

    let label = TextStyle::new(Family::Oswald, 400, 10.0, th::INK);
    let red = label.color(th::RED);
    p.text_baseline("-20", 28.0, 50.0, &label, Align::Center);
    p.text_baseline("-10", 53.0, 29.0, &label, Align::Center);
    p.text_baseline("-5", 80.0, 18.0, &label, Align::Center);
    p.text_baseline("-3", 95.0, 16.0, &label, Align::Center);
    p.text_baseline("0", 124.0, 19.0, &red, Align::Center);
    p.text_baseline("+3", 172.0, 50.0, &red, Align::Center);
    let vu = TextStyle::new(Family::Oswald, 400, 16.0, th::INK).spacing(2.0);
    p.text_baseline("VU", 100.0, 92.0, &vu, Align::Center);
}

/// Needle and pivot cap, same coordinate space as [`paint_face`].
pub fn paint_needle(p: &mut dyn Painter, db: f32) {
    let angle = angle_for(db);
    p.save();
    p.translate(PIVOT_X, PIVOT_Y);
    p.rotate(angle.to_radians());
    p.line(
        0.0,
        0.0,
        0.0,
        NEEDLE_TIP_Y - PIVOT_Y,
        Stroke::new(Color::hex(0x1E1A16), 1.6).cap(LineCap::Butt),
    );
    p.restore();
    // Pivot cap: top half of a circle at (100, 129), r = 22.
    p.begin_path();
    p.move_to(78.0, 129.0);
    p.arc(100.0, 129.0, 22.0, std::f32::consts::PI, 0.0);
    p.close_path();
    p.fill(th::INK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_table_endpoints() {
        assert_eq!(angle_for(-20.0), -50.0);
        assert_eq!(angle_for(-19.9), -49.8);
        assert!((angle_for(-30.0) - (-51.367)).abs() < 1e-3);
        assert_eq!(angle_for(f32::NEG_INFINITY), REST_ANGLE);
        assert_eq!(angle_for(f32::NAN), REST_ANGLE);
        assert_eq!(angle_for(3.0), 50.0);
        assert_eq!(angle_for(10.0), 50.0);
        assert_eq!(angle_for(0.0), 15.0);
    }

    #[test]
    fn angle_is_continuous_and_monotonic() {
        let mut prev = angle_for(-90.0);
        let mut db = -90.0;
        while db <= 5.0 {
            db += 0.01;
            let a = angle_for(db);
            assert!(a >= prev, "not monotonic at {db} dB");
            // The steepest part of the scale (+2 to +3 dB) turns 15 degrees
            // per dB; anything more over 0.01 dB is a jump.
            assert!(a - prev < 0.16, "jump of {} deg at {db} dB", a - prev);
            prev = a;
        }
    }

    #[test]
    fn angle_interpolates() {
        assert!((angle_for(-6.0) - (-16.0)).abs() < 1e-4);
        assert!((angle_for(-1.84) - 3.38).abs() < 1e-2);
    }

    #[test]
    fn scale_arc_matches_design_coordinates() {
        let (x, y) = polar(ARC_R, -50.0);
        assert!((x - 37.2).abs() < 0.1 && (y - 57.3).abs() < 0.1);
        let (x, y) = polar(ARC_R, 15.0);
        assert!((x - 121.2).abs() < 0.1 && (y - 30.8).abs() < 0.1);
        let (x, y) = polar(TICK_R_IN, 50.0);
        assert!((x - 155.2).abs() < 0.1 && (y - 63.7).abs() < 0.1);
    }
}
