//! Dominant colour of a cover, turned into a panel tint and a button accent.
//!
//! A small histogram quantiser: pixels are binned at 4 bits per channel, bins are
//! scored by population weighted towards saturated mid-lightness colours (so a
//! white page or a black frame does not win over the artwork), and the winner is
//! refined by averaging the pixels close to it. Everything is derived from that
//! single hue; the results are then clamped so text stays readable.
use iced::Color;

/// Colours derived from one cover, ready for the dark theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverColors {
    /// Opaque tint (about 30% lightness) for the top of the details panel.
    pub tint: Color,
    /// Mid-lightness accent for the primary button.
    pub accent: Color,
    /// Label colour with enough contrast on `accent`.
    pub on_accent: Color,
}

/// `theme::DARK.text_primary`, the colour of body text on the panel.
const BODY_TEXT: Color = Color::from_rgb8(232, 233, 236);
const LIGHT_TEXT: Color = Color::from_rgb8(244, 245, 248);
const DARK_TEXT: Color = Color::from_rgb8(17, 18, 20);
/// Pixels fewer than this carry no usable signal (icons, 1×1 placeholders).
const MIN_SAMPLES: usize = 64;
/// Below this saturation a pixel is grey and says nothing about hue.
/// WCAG AA for normal text.
const MIN_LABEL_CONTRAST: f32 = 4.5;
/// The button must stand out from the tinted panel behind it.
const MIN_PANEL_CONTRAST: f32 = 2.2;
const MIN_SATURATION: f32 = 0.18;

/// `None` for covers without a clear hue (greyscale, near-white or near-black).
pub fn from_rgba(width: u32, height: u32, pixels: &[u8]) -> Option<CoverColors> {
    let (width, height) = (width as usize, height as usize);
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return None;
    }
    // Every second pixel in each direction is plenty for a 192×264 thumbnail.
    let samples: Vec<[u8; 3]> = (0..height)
        .step_by(2)
        .flat_map(|y| (0..width).step_by(2).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let p = &pixels[(y * width + x) * 4..][..4];
            (p[3] >= 128).then_some([p[0], p[1], p[2]])
        })
        .collect();
    dominant(&samples).map(derive)
}

fn dominant(samples: &[[u8; 3]]) -> Option<[f32; 3]> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let mut bins = vec![(0usize, [0u32; 3]); 16 * 16 * 16];
    for pixel in samples {
        let index = (usize::from(pixel[0] >> 4) << 8)
            | (usize::from(pixel[1] >> 4) << 4)
            | usize::from(pixel[2] >> 4);
        let bin = &mut bins[index];
        bin.0 += 1;
        for (sum, channel) in bin.1.iter_mut().zip(pixel) {
            *sum += u32::from(*channel);
        }
    }
    let mut best: Option<(f32, [f32; 3])> = None;
    for (count, sum) in bins.iter().filter(|(count, _)| *count > 0) {
        let mean = sum.map(|s| s as f32 / *count as f32);
        let (_, saturation, lightness) = hsl(mean);
        if saturation < MIN_SATURATION {
            continue;
        }
        // 1 at mid lightness, falling to 0.05 at the extremes.
        let lightness_weight = (1.0 - (lightness - 0.5).abs() * 1.7).max(0.05);
        let score = *count as f32 * (0.15 + saturation) * lightness_weight;
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, mean));
        }
    }
    let (_, seed) = best?;
    // Mean-shift once: pixels near the winner pull it to the true centre of the
    // cluster instead of the centre of one quantisation cell.
    let (mut total, mut count) = ([0.0f32; 3], 0.0f32);
    for pixel in samples {
        let distance: f32 = pixel
            .iter()
            .zip(seed)
            .map(|(a, b)| (f32::from(*a) - b).powi(2))
            .sum();
        if distance < 48.0 * 48.0 {
            count += 1.0;
            for (sum, channel) in total.iter_mut().zip(pixel) {
                *sum += f32::from(*channel);
            }
        }
    }
    // The seed itself came from these samples, so `count` cannot be zero.
    Some(total.map(|sum| sum / count.max(1.0)))
}

fn derive(rgb: [f32; 3]) -> CoverColors {
    let (hue, saturation, _) = hsl(rgb);
    // Aim for 30% lightness, but yellows and greens are bright at that level:
    // darken until the body text on the panel stays readable.
    let tint_saturation = saturation.clamp(0.25, 0.5);
    let mut tint_lightness = 0.3;
    let mut tint = from_hsl(hue, tint_saturation, tint_lightness);
    while contrast(tint, BODY_TEXT) < MIN_LABEL_CONTRAST && tint_lightness > 0.1 {
        tint_lightness -= 0.01;
        tint = from_hsl(hue, tint_saturation, tint_lightness);
    }
    let saturation = saturation.clamp(0.45, 0.85);
    // Candidates closest to mid lightness first. The button needs a readable
    // label (mid-luminance colours reach 4.5:1 with neither text colour, and pure
    // blues are very dark) and must stand out from the tinted panel behind it.
    let candidates = (0..=25).map(|i| {
        let lightness = 0.5 + 0.02 * ((i + 1) / 2) as f32 * if i % 2 == 0 { 1.0 } else { -1.0 };
        let accent = from_hsl(hue, saturation, lightness);
        let on_accent = if contrast(accent, DARK_TEXT) > contrast(accent, LIGHT_TEXT) {
            DARK_TEXT
        } else {
            LIGHT_TEXT
        };
        (accent, on_accent)
    });
    let candidates: Vec<_> = candidates.collect();
    let (accent, on_accent) = candidates
        .iter()
        .find(|(accent, on)| {
            contrast(*accent, *on) >= MIN_LABEL_CONTRAST
                && contrast(*accent, tint) >= MIN_PANEL_CONTRAST
        })
        .or_else(|| {
            candidates
                .iter()
                .max_by(|(a, x), (b, y)| contrast(*a, *x).total_cmp(&contrast(*b, *y)))
        })
        .copied()
        .expect("candidates are not empty");
    CoverColors {
        tint,
        accent,
        on_accent,
    }
}

/// WCAG contrast ratio.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn luminance(color: Color) -> f32 {
    let linear = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// Hue in degrees, saturation and lightness in 0..=1; input channels in 0..=255.
fn hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta < 1e-6 {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } * 60.0;
    (hue, saturation, lightness)
}
fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> Color {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (hue / 60.0).floor().rem_euclid(6.0) as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| color(x, y))
            .collect()
    }

    #[test]
    fn hsl_roundtrip() {
        for rgb in [
            [200.0, 30.0, 60.0],
            [20.0, 180.0, 90.0],
            [40.0, 60.0, 230.0],
        ] {
            let (h, s, l) = hsl(rgb);
            let back = from_hsl(h, s, l);
            for (expected, actual) in rgb.iter().zip([back.r, back.g, back.b]) {
                assert!((expected / 255.0 - actual).abs() < 0.01, "{rgb:?}");
            }
        }
    }
    #[test]
    fn artwork_beats_white_page_and_black_frame() {
        // Mostly white margin and black frame around a red field.
        let pixels = image(100, 100, |x, y| {
            if (30..70).contains(&x) && (30..70).contains(&y) {
                [210, 40, 50, 255]
            } else if (10..90).contains(&x) && (10..90).contains(&y) {
                [5, 5, 5, 255]
            } else {
                [250, 250, 250, 255]
            }
        });
        let colors = from_rgba(100, 100, &pixels).unwrap();
        // Red hue: red channel dominates both derived colours.
        assert!(colors.tint.r > colors.tint.g && colors.tint.r > colors.tint.b);
        assert!(colors.accent.r > colors.accent.g && colors.accent.r > colors.accent.b);
    }
    #[test]
    fn greyscale_transparent_and_tiny_images_have_no_colours() {
        let grey = image(100, 100, |x, _| {
            let v = (x * 2) as u8;
            [v, v, v, 255]
        });
        assert!(from_rgba(100, 100, &grey).is_none());
        let clear = image(100, 100, |_, _| [200, 20, 20, 0]);
        assert!(from_rgba(100, 100, &clear).is_none());
        let tiny = image(4, 4, |_, _| [200, 20, 20, 255]);
        assert!(from_rgba(4, 4, &tiny).is_none());
        assert!(from_rgba(0, 0, &[]).is_none());
        assert!(from_rgba(100, 100, &[0; 8]).is_none());
    }
    #[test]
    fn derived_colours_stay_readable_for_every_hue() {
        for hue in (0..360).step_by(15) {
            for lightness in [0.2, 0.5, 0.85] {
                let base = from_hsl(hue as f32, 0.8, lightness);
                let pixels = image(80, 80, |_, _| {
                    [
                        (base.r * 255.0) as u8,
                        (base.g * 255.0) as u8,
                        (base.b * 255.0) as u8,
                        255,
                    ]
                });
                let Some(colors) = from_rgba(80, 80, &pixels) else {
                    continue;
                };
                // Body text on the tint, label on the button, button against the tint.
                assert!(contrast(colors.tint, BODY_TEXT) >= 4.5);
                assert!(contrast(colors.accent, colors.on_accent) >= 4.5);
                assert!(
                    contrast(colors.accent, colors.tint) >= 2.0,
                    "hue {hue} l {lightness}: {} {:?}",
                    contrast(colors.accent, colors.tint),
                    colors
                );
            }
        }
    }
}
