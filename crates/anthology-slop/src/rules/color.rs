//! Colour parsing for the UI rules (Task 5). Until then nothing parses.

/// A colour with channels 0..=255 and alpha 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `hsl()` and `hsla()`.
pub(crate) fn parse_color(_s: &str) -> Option<Rgba> {
    None
}

/// `(h 0..360, s 0..100, l 0..100)`.
pub(crate) fn hsl(_c: Rgba) -> (f64, f64, f64) {
    (0.0, 0.0, 0.0)
}

/// h in 228..=300, s >= 40, 15 < l < 90.
pub(crate) fn is_violet(_c: Rgba) -> bool {
    false
}

/// Every colour written in `s`.
pub(crate) fn colors_in(_s: &str) -> Vec<Rgba> {
    Vec::new()
}
