//! Colour parsing for the UI rules: just enough CSS colour syntax to tell a violet from a blue.

use regex::Regex;
use std::sync::LazyLock;

/// A colour with channels 0..=255 and alpha 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

/// A hex run, or an `rgb`/`rgba`/`hsl`/`hsla` call without nested parentheses.
static COLOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?)\([^()]*\)").expect("colour regex")
});

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `hsl()` and `hsla()`.
pub(crate) fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = s.to_ascii_lowercase();
    let (name, rest) = lower.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    match name {
        "rgb" | "rgba" => parse_rgb(args),
        "hsl" | "hsla" => parse_hsl(args),
        _ => None,
    }
}

/// `(h 0..360, s 0..100, l 0..100)`.
pub(crate) fn hsl(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (c.r / 255.0, c.g / 255.0, c.b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= 0.0 {
        return (0.0, 0.0, l * 100.0);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let sector = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (sector * 60.0, s * 100.0, l * 100.0)
}

/// h in 228..=300, s >= 40, 15 < l < 90.
pub(crate) fn is_violet(c: Rgba) -> bool {
    is_chromatic(c) && (228.0..=300.0).contains(&hsl(c).0)
}

/// s >= 40 and 15 < l < 90: a colour with a hue to speak of, not a grey, a near-white or a
/// near-black.
pub(crate) fn is_chromatic(c: Rgba) -> bool {
    let (_, s, l) = hsl(c);
    s >= 40.0 && l > 15.0 && l < 90.0
}

/// Violet, indigo, purple or pink-violet: chromatic, h in 228..=330.
pub(crate) fn is_violet_family(c: Rgba) -> bool {
    is_chromatic(c) && (228.0..=330.0).contains(&hsl(c).0)
}

/// The colours a violet is paired with in the stock gradient: chromatic, h in 190..=345 (blue
/// through pink).
pub(crate) fn is_blue_to_pink(c: Rgba) -> bool {
    is_chromatic(c) && (190.0..=345.0).contains(&hsl(c).0)
}

/// Every colour written in `s`, in order. Text that is not a valid colour is skipped.
pub(crate) fn colors_in(s: &str) -> Vec<Rgba> {
    COLOR
        .find_iter(s)
        .filter_map(|m| parse_color(m.as_str()))
        .collect()
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let n: Vec<f64> = hex
        .chars()
        .map(|c| c.to_digit(16).map(f64::from))
        .collect::<Option<_>>()?;
    let (r, g, b, a) = match n.len() {
        3 => (n[0] * 17.0, n[1] * 17.0, n[2] * 17.0, 255.0),
        4 => (n[0] * 17.0, n[1] * 17.0, n[2] * 17.0, n[3] * 17.0),
        6 => (
            n[0] * 16.0 + n[1],
            n[2] * 16.0 + n[3],
            n[4] * 16.0 + n[5],
            255.0,
        ),
        8 => (
            n[0] * 16.0 + n[1],
            n[2] * 16.0 + n[3],
            n[4] * 16.0 + n[5],
            n[6] * 16.0 + n[7],
        ),
        _ => return None,
    };
    Some(Rgba {
        r,
        g,
        b,
        a: a / 255.0,
    })
}

/// The arguments of a colour function: three values, then an optional alpha, split on commas,
/// slashes and white space.
fn tokens(args: &str) -> Option<Vec<&str>> {
    let t: Vec<&str> = args
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    (3..=4).contains(&t.len()).then_some(t)
}

/// A number, or a percentage of `full`. Never NaN or infinite.
fn component(tok: &str, full: f64) -> Option<f64> {
    let v = match tok.strip_suffix('%') {
        Some(p) => p.parse::<f64>().ok()? / 100.0 * full,
        None => tok.parse::<f64>().ok()?,
    };
    v.is_finite().then_some(v)
}

fn parse_rgb(args: &str) -> Option<Rgba> {
    let t = tokens(args)?;
    let channel = |i: usize| component(t[i], 255.0).map(|v| v.clamp(0.0, 255.0));
    Some(Rgba {
        r: channel(0)?,
        g: channel(1)?,
        b: channel(2)?,
        a: alpha(&t)?,
    })
}

fn parse_hsl(args: &str) -> Option<Rgba> {
    let t = tokens(args)?;
    let h = hue(t[0])?.rem_euclid(360.0);
    let s = component(t[1], 100.0)?.clamp(0.0, 100.0) / 100.0;
    let l = component(t[2], 100.0)?.clamp(0.0, 100.0) / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Some(Rgba {
        r: (r + m) * 255.0,
        g: (g + m) * 255.0,
        b: (b + m) * 255.0,
        a: alpha(&t)?,
    })
}

/// The fourth token as 0..=1, or 1 when there is none.
fn alpha(t: &[&str]) -> Option<f64> {
    match t.get(3) {
        Some(tok) => component(tok, 1.0).map(|v| v.clamp(0.0, 1.0)),
        None => Some(1.0),
    }
}

/// A hue in degrees: a bare number, or one with `deg`, `grad`, `rad` or `turn`.
fn hue(tok: &str) -> Option<f64> {
    let (num, per_unit) = if let Some(n) = tok.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = tok.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = tok.strip_suffix("rad") {
        (n, 180.0 / std::f64::consts::PI)
    } else if let Some(n) = tok.strip_suffix("turn") {
        (n, 360.0)
    } else {
        (tok, 1.0)
    };
    let v = num.parse::<f64>().ok()? * per_unit;
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(c: Option<Rgba>, want: (f64, f64, f64, f64)) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() < 0.01;
        c.is_some_and(|c| {
            close(c.r, want.0) && close(c.g, want.1) && close(c.b, want.2) && close(c.a, want.3)
        })
    }

    #[test]
    fn parses_every_colour_form() {
        assert!(is(parse_color("#fff"), (255.0, 255.0, 255.0, 1.0)));
        assert!(is(parse_color("#0f08"), (0.0, 255.0, 0.0, 136.0 / 255.0)));
        assert!(is(parse_color("#6366f1"), (99.0, 102.0, 241.0, 1.0)));
        assert!(is(parse_color("  #6366F1  "), (99.0, 102.0, 241.0, 1.0)));
        assert!(is(
            parse_color("#6366f180"),
            (99.0, 102.0, 241.0, 128.0 / 255.0)
        ));
        assert!(is(
            parse_color("rgb(99, 102, 241)"),
            (99.0, 102.0, 241.0, 1.0)
        ));
        assert!(is(
            parse_color("rgb(99 102 241)"),
            (99.0, 102.0, 241.0, 1.0)
        ));
        assert!(is(
            parse_color("RGBA(99,102,241,0.5)"),
            (99.0, 102.0, 241.0, 0.5)
        ));
        assert!(is(
            parse_color("rgb(99 102 241 / 50%)"),
            (99.0, 102.0, 241.0, 0.5)
        ));
        assert!(is(
            parse_color("rgb(99 102 241 / .25)"),
            (99.0, 102.0, 241.0, 0.25)
        ));
        assert!(is(parse_color("rgb(100% 0% 0%)"), (255.0, 0.0, 0.0, 1.0)));
        assert!(is(parse_color("rgb(300, -4, 0)"), (255.0, 0.0, 0.0, 1.0)));
        assert!(is(
            parse_color("hsl(240, 100%, 50%)"),
            (0.0, 0.0, 255.0, 1.0)
        ));
        assert!(is(
            parse_color("hsla(240deg 100% 50% / 0.25)"),
            (0.0, 0.0, 255.0, 0.25)
        ));
        assert!(is(
            parse_color("hsl(0.5turn 100% 50%)"),
            (0.0, 255.0, 255.0, 1.0)
        ));
        assert!(is(
            parse_color("hsl(-120 100% 50%)"),
            (0.0, 0.0, 255.0, 1.0)
        ));
        assert!(is(parse_color("hsl(120 100 25)"), (0.0, 127.5, 0.0, 1.0)));
    }

    #[test]
    fn rejects_what_is_not_a_colour() {
        for s in [
            "",
            "#",
            "#ff",
            "#fffff",
            "#fffffff",
            "#ggg",
            "blue",
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4, 5)",
            "rgb(a, b, c)",
            "rgb(nan, 1, 2)",
            "rgb(1, 2, 3",
            "rgb (1, 2, 3)",
            "hsl(1, 2%)",
            "hsl(10xyz 50% 50%)",
            "oklch(0.5 0.2 30)",
            "var(--x)",
            "#6366f1 0%",
        ] {
            assert!(parse_color(s).is_none(), "{s:?} parsed");
        }
    }

    #[test]
    fn hsl_reads_hue_saturation_and_lightness() {
        let (h, s, l) = hsl(parse_color("#ff0000").unwrap());
        assert!(h.abs() < 0.01 && (s - 100.0).abs() < 0.01 && (l - 50.0).abs() < 0.01);
        let (h, s, l) = hsl(parse_color("#808080").unwrap());
        assert!(h == 0.0 && s == 0.0 && (l - 50.2).abs() < 0.1);
        let (h, _, _) = hsl(parse_color("#ff0080").unwrap());
        assert!((h - 330.0).abs() < 0.5, "wrapped hue {h}");
    }

    #[test]
    fn indigo_500_is_violet_and_blue_500_is_not() {
        let indigo = parse_color("#6366f1").unwrap();
        let (h, _, _) = hsl(indigo);
        assert!((h - 239.0).abs() < 1.0, "indigo hue {h}");
        assert!(is_violet(indigo));
        let blue = parse_color("#3b82f6").unwrap();
        let (h, _, _) = hsl(blue);
        assert!((h - 217.0).abs() < 1.0, "blue hue {h}");
        assert!(!is_violet(blue));
        assert!(is_violet(parse_color("#a855f7").unwrap()));
        assert!(is_violet(parse_color("#7c3aed").unwrap()));
    }

    #[test]
    fn violet_needs_the_hue_saturation_and_lightness_band() {
        let violet = |s: &str| is_violet(parse_color(s).unwrap());
        assert!(violet("hsl(230 50% 50%)") && violet("hsl(298 50% 50%)"));
        assert!(!violet("hsl(226 50% 50%)") && !violet("hsl(304 50% 50%)"));
        assert!(violet("hsl(260 45% 50%)") && !violet("hsl(260 30% 50%)"));
        assert!(violet("hsl(260 50% 20%)") && !violet("hsl(260 50% 10%)"));
        assert!(violet("hsl(260 50% 85%)") && !violet("hsl(260 50% 95%)"));
        assert!(!violet("#808080"), "a grey has no hue");
    }

    #[test]
    fn chromatic_means_saturated_and_neither_near_white_nor_near_black() {
        let chromatic = |s: &str| is_chromatic(parse_color(s).unwrap());
        assert!(chromatic("#3b82f6") && chromatic("#22c55e") && chromatic("#f97316"));
        assert!(!chromatic("#ffffff") && !chromatic("#000000") && !chromatic("#808080"));
        assert!(!chromatic("#23272a"), "a near-black has no hue to speak of");
        assert!(!chromatic("hsl(260 30% 50%)") && !chromatic("hsl(260 50% 95%)"));
    }

    #[test]
    fn the_violet_family_runs_from_indigo_to_pink_violet() {
        let family = |s: &str| is_violet_family(parse_color(s).unwrap());
        assert!(family("#6366f1") && family("#a855f7") && family("#d946ef"));
        assert!(family("hsl(230 80% 50%)") && family("hsl(328 80% 50%)"));
        assert!(!family("hsl(226 80% 50%)") && !family("hsl(334 80% 50%)"));
        assert!(
            !family("#3b82f6") && !family("#ec4899"),
            "blue and hot pink are outside"
        );
        assert!(!family("hsl(270 20% 50%)"), "a grey-purple is no colour");
    }

    #[test]
    fn blue_to_pink_is_the_partner_range_of_a_violet() {
        let partner = |s: &str| is_blue_to_pink(parse_color(s).unwrap());
        assert!(partner("#3b82f6") && partner("#6366f1") && partner("#ec4899"));
        assert!(partner("hsl(192 80% 50%)") && partner("hsl(343 80% 50%)"));
        assert!(!partner("hsl(186 80% 50%)") && !partner("hsl(348 80% 50%)"));
        assert!(!partner("#22c55e") && !partner("#f97316") && !partner("#ffffff"));
    }

    #[test]
    fn colors_in_finds_every_colour_in_source_order() {
        let found = colors_in(
            "135deg, #6366f1 0%, rgba(168, 85, 247, 0.5) 50%, hsl(262 83% 58%), #abc, var(--x), #12345",
        );
        assert_eq!(found.len(), 4);
        assert!(is(Some(found[0]), (99.0, 102.0, 241.0, 1.0)));
        assert!(is(Some(found[1]), (168.0, 85.0, 247.0, 0.5)));
        assert!(is_violet(found[2]));
        assert!(is(Some(found[3]), (170.0, 187.0, 204.0, 1.0)));
        assert!(colors_in("to right, red, blue").is_empty());
        assert!(
            colors_in("#6366f1x").is_empty(),
            "a hex run must end at a boundary"
        );
    }
}
