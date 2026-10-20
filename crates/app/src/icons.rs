//! App artwork and state-aware status glyphs, drawn only on the UI thread.

use crate::tray::TrayState;

pub fn app_icon() -> eframe::egui::IconData {
    let bytes = include_bytes!("../../../assets/icon/png/voice-strip-256.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("embedded icon header");
    let mut rgba = vec![0; reader.output_buffer_size().expect("icon buffer size")];
    let info = reader.next_frame(&mut rgba).expect("embedded icon pixels");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    rgba.truncate(info.buffer_size());
    eframe::egui::IconData {
        rgba,
        width: info.width,
        height: info.height,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    On,
    Idle,
    Muted,
}

impl Status {
    pub fn of(state: TrayState) -> Self {
        if state.muted {
            Self::Muted
        } else if !state.on || state.standby {
            Self::Idle
        } else {
            Self::On
        }
    }
}

/// RGBA pixels. Template images use only alpha; Linux gets a dark outline
/// so the palette colours remain visible on both light and dark panels.
pub fn glyph(size: u32, status: Status, template: bool) -> Vec<u8> {
    const BARS: [(f32, f32, f32, f32); 5] = [
        (2., 17., 8., 19.),
        (10., 11., 13., 25.),
        (17., 5., 20., 31.),
        (24., 11., 27., 25.),
        (29., 17., 34., 19.),
    ];
    let inside = |x: f32, y: f32, pad: f32| {
        BARS.iter()
            .any(|&(x0, y0, x1, y1)| x >= x0 - pad && x < x1 + pad && y >= y0 - pad && y < y1 + pad)
            || (status == Status::Muted
                && (x + y - 36.).abs() < 1.6 + pad
                && (4. - pad..32. + pad).contains(&x)
                && (4. - pad..32. + pad).contains(&y))
    };
    let color = if template {
        [0, 0, 0]
    } else {
        match status {
            Status::On => [0xF0, 0xA2, 0x3B],
            Status::Idle => [0xA0, 0xA8, 0xB3],
            Status::Muted => [0xD2, 0x55, 0x3A],
        }
    };
    let alpha = if template && status == Status::Idle {
        110
    } else {
        255
    };
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0u32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 4.) * 36. / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / 4.) * 36. / size as f32;
                    let (rgb, a) = if inside(px, py, 0.) {
                        (color, alpha)
                    } else if !template && inside(px, py, 0.8) {
                        ([0x0E, 0x10, 0x13], 255)
                    } else {
                        ([0, 0, 0], 0)
                    };
                    for i in 0..3 {
                        sum[i] += rgb[i] * a;
                    }
                    sum[3] += a;
                }
            }
            for value in &sum[..3] {
                rgba.push(value.checked_div(sum[3]).unwrap_or(0) as u8);
            }
            rgba.push((sum[3] / 16) as u8);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_is_rgba_256() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert_eq!(icon.rgba[3], 0);
    }

    #[test]
    fn template_states_have_distinct_alpha() {
        for size in [18, 36] {
            let on = glyph(size, Status::On, true);
            let idle = glyph(size, Status::Idle, true);
            let muted = glyph(size, Status::Muted, true);
            assert_eq!(on.len(), (size * size * 4) as usize);
            assert!(
                idle.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| p[3] as u32)
                    .sum::<u32>()
                    < on.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[3] as u32)
                        .sum::<u32>()
            );
            assert_ne!(on, muted);
        }
    }
}
