//! The tray icon, drawn at the size the notification area or the menu bar
//! shows it.
//!
//! The mark is the application's seven-bar waveform, with the bar heights and
//! duty measured in `assets/icons/derive.py`. It is drawn here rather than
//! shipped as bitmaps because every state needs a frame at every display scale,
//! and a frame resampled to another size is the blur the drawn frames exist to
//! avoid. Bars sit on whole pixels, so they stay crisp at 16 px; only their
//! rounded ends and the badge are anti-aliased.

/// What the icon says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Idle,
    /// The microphone is open or opening: a red dot. A change of shape rather
    /// than of tint, so it reads on a light and a dark taskbar alike.
    Recording,
    /// A dictation is being transcribed: every other bar dims.
    Working,
    /// The last dictation failed and nobody has dismissed it: an amber dot.
    Attention,
}

/// Bar heights relative to the tallest, and bar width as a share of the
/// bar-to-bar period, both measured off the master artwork by `derive.py`.
const BAR_HEIGHTS: [f64; 7] = [0.198, 0.385, 0.658, 1.0, 0.608, 0.368, 0.190];
const BAR_DUTY: f64 = 0.616;
/// The mark's terracotta. It holds against both a white and a black taskbar,
/// which a pure black or white silhouette cannot.
const ACCENT: [u8; 3] = [178, 91, 41];
const RECORDING: [u8; 3] = [229, 72, 77];
const ATTENTION: [u8; 3] = [245, 165, 36];
/// How much of the icon the badge spans, and the clear ring that keeps it
/// apart from the bars it overlaps.
const DOT: f64 = 0.4;
/// Sub-samples per pixel along each axis, as `derive.py` supersamples.
const SAMPLES: u32 = 8;

/// Whether `look` needs its colours. macOS tints a template image to suit the
/// menu bar, which would turn a red dot grey, so only the other looks are drawn
/// as templates there.
pub fn coloured(look: Look) -> bool {
    matches!(look, Look::Recording | Look::Attention)
}

/// The RGBA pixels of a `size` × `size` icon, row by row. A `template` icon is
/// black: macOS reads only its alpha.
pub fn draw(size: u32, look: Look, template: bool) -> Vec<u8> {
    let size = size.clamp(8, 256);
    let ink = if template { [0; 3] } else { ACCENT };
    let badge = match look {
        Look::Recording if !template => Some(RECORDING),
        Look::Attention if !template => Some(ATTENTION),
        Look::Recording | Look::Attention => Some([0; 3]),
        Look::Idle | Look::Working => None,
    };
    let radius = f64::from(size) * DOT / 2.0;
    let centre = f64::from(size) - radius;
    let ring = (f64::from(size) / 16.0).max(1.0);
    let bars = bars(size);

    let mut pixels = vec![0; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let mut under = 0.0;
            for (index, bar) in bars.iter().enumerate() {
                let weight = if look == Look::Working && index % 2 == 1 {
                    0.45
                } else {
                    1.0
                };
                under += coverage(x, y, |px, py| bar.contains(px, py)) * weight;
            }
            let mut over = 0.0;
            if badge.is_some() {
                let inside =
                    |limit: f64| move |px: f64, py: f64| (px - centre).hypot(py - centre) <= limit;
                under *= 1.0 - coverage(x, y, inside(radius + ring));
                over = coverage(x, y, inside(radius));
            }
            let alpha = over + under * (1.0 - over);
            if alpha <= 0.0 {
                continue;
            }
            let colour = badge.unwrap_or(ink);
            let at = ((y * size + x) * 4) as usize;
            for channel in 0..3 {
                let mixed = (f64::from(colour[channel]) * over
                    + f64::from(ink[channel]) * under * (1.0 - over))
                    / alpha;
                pixels[at + channel] = mixed.round() as u8;
            }
            pixels[at + 3] = (alpha * 255.0).round().min(255.0) as u8;
        }
    }
    pixels
}

/// One bar, in whole pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bar {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Bar {
    /// Inside the bar with its ends rounded to half its width.
    fn contains(&self, px: f64, py: f64) -> bool {
        let (left, top) = (f64::from(self.x), f64::from(self.y));
        let (right, bottom) = (left + f64::from(self.width), top + f64::from(self.height));
        if px < left || px > right || py < top || py > bottom {
            return false;
        }
        let r = f64::from(self.width.min(self.height)) / 2.0;
        let nearest_x = px.clamp(left + r, right - r);
        let nearest_y = py.clamp(top + r, bottom - r);
        (px - nearest_x).hypot(py - nearest_y) <= r
    }
}

/// The bars for an icon of `size` pixels: the widest whole-pixel bars whose
/// seven bars and six gaps fit inside the padding, centred.
fn bars(size: u32) -> Vec<Bar> {
    let count = BAR_HEIGHTS.len() as u32;
    let pad = (f64::from(size) * 0.08).round() as u32;
    let content = size - 2 * pad;
    let ratio = (1.0 - BAR_DUTY) / BAR_DUTY;
    let gap_for = |width: u32| ((f64::from(width) * ratio).round() as u32).max(1);
    let span = |width: u32, gap: u32| count * width + (count - 1) * gap;
    let mut width = (content / count)
        .min(
            (f64::from(content) / (f64::from(count) + f64::from(count - 1) * ratio)).round() as u32,
        )
        .max(1);
    let mut gap = gap_for(width);
    // Narrower gaps before narrower bars, so the mark keeps its size.
    while span(width, gap) > content && gap > 1 {
        gap -= 1;
    }
    while span(width, gap) > content && width > 1 {
        width -= 1;
        gap = gap.min(gap_for(width));
    }
    let left = (size - span(width, gap)) / 2;
    BAR_HEIGHTS
        .iter()
        .enumerate()
        .map(|(index, ratio)| {
            let height = ((f64::from(content) * ratio).round() as u32).max(1);
            Bar {
                x: left + index as u32 * (width + gap),
                y: (size - height) / 2,
                width,
                height,
            }
        })
        .collect()
}

/// The share of pixel (`x`, `y`) inside a shape, from a grid of sub-samples.
fn coverage(x: u32, y: u32, inside: impl Fn(f64, f64) -> bool) -> f64 {
    let step = 1.0 / f64::from(SAMPLES);
    let mut hits = 0;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let px = f64::from(x) + (f64::from(sx) + 0.5) * step;
            let py = f64::from(y) + (f64::from(sy) + 0.5) * step;
            if inside(px, py) {
                hits += 1;
            }
        }
    }
    f64::from(hits) / f64::from(SAMPLES * SAMPLES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) * 4 + 3) as usize]
    }

    /// The 16, 20, 24 and 32 px frames `derive.py` drew into the tray icon
    /// file, as (size, alpha by row). The runtime drawing has to match them, or
    /// the two descriptions of the mark have drifted apart.
    fn reference_frames() -> Vec<(u32, Vec<u8>)> {
        let ico = include_bytes!("../../../../assets/icons/writer-tray.ico");
        let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
        let long = |at: usize| u32::from_le_bytes(ico[at..at + 4].try_into().unwrap());
        (0..usize::from(word(4)))
            .map(|index| {
                let entry = 6 + 16 * index;
                let size = u32::from(ico[entry]);
                let offset = long(entry + 12) as usize;
                let header = long(offset) as usize;
                let colour = offset + header;
                // 32-bit DIB rows run bottom-up, BGRA.
                let mut rows = vec![0; (size * size) as usize];
                for y in 0..size {
                    for x in 0..size {
                        let at = colour + (((size - 1 - y) * size + x) * 4) as usize;
                        rows[(y * size + x) as usize] = ico[at + 3];
                    }
                }
                (size, rows)
            })
            .collect()
    }

    #[test]
    fn the_idle_mark_matches_the_frames_derive_py_drew() {
        let frames = reference_frames();
        assert_eq!(
            frames.iter().map(|(size, _)| *size).collect::<Vec<_>>(),
            [16, 20, 24, 32]
        );
        for (size, expected) in frames {
            let drawn = draw(size, Look::Idle, false);
            for y in 0..size {
                for x in 0..size {
                    let want = expected[(y * size + x) as usize];
                    let got = alpha(&drawn, size, x, y);
                    // Only the anti-aliased ends may differ, and only slightly:
                    // a bar in a different column or of a different height is
                    // a full 255 apart.
                    assert!(
                        want.abs_diff(got) <= 40,
                        "{size} px at ({x}, {y}): derive.py {want}, drawn {got}"
                    );
                }
            }
        }
    }

    #[test]
    fn bars_stay_on_whole_pixels_and_inside_the_icon_at_every_scale() {
        for size in [16, 18, 20, 24, 28, 32, 36, 40, 48, 64] {
            let bars = bars(size);
            assert_eq!(bars.len(), 7);
            let last = bars.last().unwrap();
            assert!(
                bars[0].x >= 1 && last.x + last.width < size,
                "{size}: {bars:?}"
            );
            // The tallest bar is the fourth, as in the artwork.
            assert!(bars.iter().all(|bar| bar.height <= bars[3].height));
            // Wider icons get wider bars rather than a smaller mark.
            let span = last.x + last.width - bars[0].x;
            assert!(f64::from(span) >= f64::from(size) * 0.6, "{size}: {span}");
        }
    }

    #[test]
    fn every_state_is_a_different_shape_not_only_a_different_colour() {
        let size = 16;
        let idle = draw(size, Look::Idle, true);
        for look in [Look::Recording, Look::Working, Look::Attention] {
            let other = draw(size, look, true);
            let changed = (0..size * size)
                .filter(|index| {
                    idle[(*index * 4 + 3) as usize].abs_diff(other[(*index * 4 + 3) as usize]) > 64
                })
                .count();
            assert!(changed >= 6, "{look:?} changes only {changed} pixels");
        }
        // The two badged looks share a shape and differ in colour, which is why
        // they are never drawn as macOS templates.
        assert!(coloured(Look::Recording) && coloured(Look::Attention));
        assert!(!coloured(Look::Idle) && !coloured(Look::Working));
        let recording = draw(size, Look::Recording, false);
        let attention = draw(size, Look::Attention, false);
        let corner = (((size - 3) * size + size - 3) * 4) as usize;
        assert_eq!(&recording[corner..corner + 4], &[229, 72, 77, 255]);
        assert_eq!(&attention[corner..corner + 4], &[245, 165, 36, 255]);
    }

    #[test]
    fn the_badge_is_kept_apart_from_the_bars_by_a_clear_ring() {
        for size in [16, 20, 24, 32, 36] {
            let pixels = draw(size, Look::Recording, false);
            let radius = f64::from(size) * DOT / 2.0;
            let centre = f64::from(size) - radius;
            let ring = (f64::from(size) / 16.0).max(1.0);
            for y in 0..size {
                for x in 0..size {
                    let distance = (f64::from(x) + 0.5 - centre).hypot(f64::from(y) + 0.5 - centre);
                    // Pixels wholly inside the ring are see-through.
                    if distance > radius + 0.75 && distance < radius + ring - 0.75 {
                        assert_eq!(alpha(&pixels, size, x, y), 0, "{size} px at ({x}, {y})");
                    }
                }
            }
        }
    }

    #[test]
    fn a_template_is_black_and_working_dims_alternate_bars() {
        let template = draw(24, Look::Working, true);
        assert!(template.chunks(4).all(|pixel| pixel[..3] == [0, 0, 0]));
        let bars = bars(24);
        let middle = |bar: &Bar| alpha(&template, 24, bar.x, bar.y + bar.height / 2);
        assert_eq!(middle(&bars[0]), 255);
        assert_eq!(middle(&bars[1]), (0.45_f64 * 255.0).round() as u8);
        assert_eq!(middle(&bars[2]), 255);
        assert_eq!(middle(&bars[3]), (0.45_f64 * 255.0).round() as u8);
    }
}
