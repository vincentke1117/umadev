//! Pure-Rust color math for the design-system floor (UD-CODE-007).
//!
//! Everything the deterministic design checks need to reason about color
//! WITHOUT a browser, a headless renderer, or a single new dependency:
//!
//! - [`parse_color`] — one entry point that understands the color syntaxes a
//!   real token file actually ships: `#rgb` / `#rrggbb` / `#rrggbbaa`,
//!   `rgb()` / `rgba()` (legacy-comma AND space-separated), and `oklch()`.
//! - [`Srgb::luminance`] / [`contrast_ratio`] — the WCAG 2.x relative-luminance
//!   and contrast formulas, so a declared `(surface, on-surface)` pair can be
//!   MEASURED rather than eyeballed. 4.5:1 is the body-text floor, 3:1 the
//!   large-text / UI floor.
//! - [`Srgb::oklch`] / [`is_ai_purple`] — the perceptual (OKLCH) coordinates,
//!   used to recognize the AI indigo/violet band as a *band* (hue 270–320 at
//!   chroma ≥ 0.09) instead of an ever-growing hard-coded hex list, so a
//!   near-neighbour of a banned hex is caught too — while genuine blues (which
//!   land below hue 270 in OKLCH) are deliberately NOT caught.
//!
//! **Fail-open by contract** (like every governance function): an
//! unparseable/exotic color yields `None`, never an error and never a panic.
//! A caller that cannot parse a color simply has nothing to say about it.

/// A color resolved into sRGB, plus its alpha. Channels are `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Srgb {
    /// Red channel, `0.0..=1.0`.
    pub r: f64,
    /// Green channel, `0.0..=1.0`.
    pub g: f64,
    /// Blue channel, `0.0..=1.0`.
    pub b: f64,
    /// Alpha, `0.0..=1.0` (`1.0` when the source declared none).
    pub a: f64,
}

/// A color in OKLCH — perceptual lightness, chroma, hue (degrees `0..360`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    /// Perceptual lightness, `0.0..=1.0`.
    pub l: f64,
    /// Chroma (colorfulness); `0.0` is a pure gray. Typical brand hues: `0.1..0.3`.
    pub c: f64,
    /// Hue angle in degrees, `0.0..360.0`.
    pub h: f64,
}

/// sRGB gamma decode: one channel `0.0..=1.0` → linear light.
fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB gamma encode: one linear-light channel → `0.0..=1.0` (gamut-clipped).
fn linear_to_srgb(channel: f64) -> f64 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.003_130_8 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

impl Srgb {
    /// WCAG 2.x relative luminance of this color (alpha ignored — a pair is
    /// measured on its own declared values, which is what a token file states).
    #[must_use]
    pub fn luminance(self) -> f64 {
        0.2126 * srgb_to_linear(self.r)
            + 0.7152 * srgb_to_linear(self.g)
            + 0.0722 * srgb_to_linear(self.b)
    }

    /// This color's OKLCH coordinates.
    #[must_use]
    pub fn oklch(self) -> Oklch {
        let (red, green, blue) = (
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
        );
        // Linear sRGB → LMS cone response.
        let cone_l = 0.412_221_470_8 * red + 0.536_332_536_3 * green + 0.051_445_992_9 * blue;
        let cone_m = 0.211_903_498_2 * red + 0.680_699_545_1 * green + 0.107_396_956_6 * blue;
        let cone_s = 0.088_302_461_9 * red + 0.281_718_837_6 * green + 0.629_978_700_5 * blue;
        let cbrt = |v: f64| v.abs().cbrt().copysign(v);
        let (root_l, root_m, root_s) = (cbrt(cone_l), cbrt(cone_m), cbrt(cone_s));
        // LMS' → OKLab.
        let lightness =
            0.210_454_255_3 * root_l + 0.793_617_785_0 * root_m - 0.004_072_046_8 * root_s;
        let green_red =
            1.977_998_495_1 * root_l - 2.428_592_205_0 * root_m + 0.450_593_709_9 * root_s;
        let blue_yellow =
            0.025_904_037_1 * root_l + 0.782_771_766_2 * root_m - 0.808_675_766_0 * root_s;
        // OKLab → OKLCH (polar).
        Oklch {
            l: lightness,
            c: green_red.hypot(blue_yellow),
            h: blue_yellow.atan2(green_red).to_degrees().rem_euclid(360.0),
        }
    }

    /// Per-channel 0–255 form, for tolerance comparisons against a token set.
    #[must_use]
    pub fn to_u8(self) -> (u8, u8, u8) {
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (q(self.r), q(self.g), q(self.b))
    }

    /// Whether two colors are the SAME within a per-channel tolerance (0–255
    /// units). Used to decide whether a literal in source was "drawn from" the
    /// token set — a token rendered through an opacity/hover tweak lands a few
    /// units away, and we do not want to cry drift over that.
    #[must_use]
    pub fn near(self, other: Self, tol: u8) -> bool {
        let (r1, g1, b1) = self.to_u8();
        let (r2, g2, b2) = other.to_u8();
        let d = |a: u8, b: u8| a.abs_diff(b);
        d(r1, r2) <= tol && d(g1, g2) <= tol && d(b1, b2) <= tol
    }
}

/// WCAG 2.x contrast ratio between two colors — `1.0..=21.0`.
/// `>= 4.5` passes AA body text; `>= 3.0` passes AA large text / UI components.
#[must_use]
pub fn contrast_ratio(a: Srgb, b: Srgb) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The AI indigo/violet band, expressed perceptually rather than as a hex list:
/// OKLCH **hue 270–320** at **chroma ≥ 0.09** and **lightness 0.35–0.85**.
///
/// This is exactly the band the canonical tells live in (`#6366f1` h≈277,
/// `#4f46e5` h≈277, `#8b5cf6` h≈293, `#7c3aed` h≈293, `#a855f7` h≈304) plus
/// their near neighbours — while real blues (`#2563eb` h≈263, `#5e8bff` h≈266,
/// even pure `#0000ff` h≈264) sit BELOW 270 and are deliberately not flagged.
/// The chroma floor keeps a near-gray lavender tint out; the lightness window
/// keeps a near-black/near-white tint out (those are surfaces, not brand hues).
#[must_use]
pub fn is_ai_purple(c: Srgb) -> bool {
    let o = c.oklch();
    (270.0..=320.0).contains(&o.h) && o.c >= 0.09 && (0.35..=0.85).contains(&o.l)
}

/// The rose / pink / fuchsia band — the OTHER end of the classic AI hero gradient,
/// expressed perceptually rather than as a hex list: OKLCH **hue > 320 wrapping through
/// 20** at **chroma ≥ 0.09** and **lightness 0.35–0.90**.
///
/// A hex list is exactly how this check was side-stepped: it knew `#ec4899` and `#f472b6`
/// and nothing else, so `linear-gradient(#7c3aed, #db2777)` (h≈0.6) and
/// `linear-gradient(#7c3aed, #f43f5e)` (h≈16) — two of the most common purple→pink heroes
/// there are — did not block. The band covers the whole family: fuchsia (`#d946ef` h≈322),
/// magenta (h≈328), pink (`#ec4899` h≈354, `#db2777` h≈0.6), rose (`#f43f5e` h≈16,
/// `#e11d48` h≈18) — while true reds (`#ef4444` h≈25, `#dc2626` h≈27) and every warmer hue
/// sit outside it. The chroma floor keeps a near-gray blush out; the lightness window keeps
/// near-black/near-white tints out (those are surfaces, not brand hues).
///
/// Deliberately starts just ABOVE [`is_ai_purple`]'s 320° ceiling so the two bands partition
/// the hue circle instead of overlapping — a single hue can never read as both ends of a
/// two-stop gradient.
#[must_use]
pub fn is_ai_pink(c: Srgb) -> bool {
    let o = c.oklch();
    let in_hue = o.h > 320.0 || o.h <= 20.0;
    in_hue && o.c >= 0.09 && (0.35..=0.90).contains(&o.l)
}

/// Parse ONE color from a CSS value fragment. Understands `#rgb`, `#rrggbb`,
/// `#rrggbbaa`, `rgb()` / `rgba()` (comma or space separated, `%` or 0–255),
/// and `oklch()` (`L` as `0..1` or a percentage; `C`; `H` in degrees; optional
/// `/ alpha`). Returns `None` for anything else — `var(...)`, a named color, a
/// gradient, `currentColor` — which is the fail-open path: an unrecognised
/// color simply carries no judgment.
#[must_use]
pub fn parse_color(value: &str) -> Option<Srgb> {
    let v = value.trim().trim_end_matches(';').trim();
    let lower = v.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = strip_fn(&lower, "rgba").or_else(|| strip_fn(&lower, "rgb")) {
        return parse_rgb_fn(inner);
    }
    if let Some(inner) = strip_fn(&lower, "oklch") {
        return parse_oklch_fn(inner);
    }
    None
}

/// `name(...)` → the inner text, if `lower` is exactly that call.
fn strip_fn<'a>(lower: &'a str, name: &str) -> Option<&'a str> {
    let rest = lower.strip_prefix(name)?.trim_start();
    let inner = rest.strip_prefix('(')?;
    let end = inner.find(')')?;
    Some(&inner[..end])
}

/// `#rgb` / `#rrggbb` / `#rrggbbaa` (the leading `#` already stripped).
fn parse_hex(hex: &str) -> Option<Srgb> {
    let digits = hex.trim();
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte_at = |s: &str| u8::from_str_radix(s, 16).ok().map(f64::from);
    let (red, green, blue, alpha) = match digits.len() {
        3 => {
            let short: Vec<char> = digits.chars().collect();
            (
                byte_at(&format!("{}{}", short[0], short[0]))?,
                byte_at(&format!("{}{}", short[1], short[1]))?,
                byte_at(&format!("{}{}", short[2], short[2]))?,
                255.0,
            )
        }
        6 => (
            byte_at(&digits[0..2])?,
            byte_at(&digits[2..4])?,
            byte_at(&digits[4..6])?,
            255.0,
        ),
        8 => (
            byte_at(&digits[0..2])?,
            byte_at(&digits[2..4])?,
            byte_at(&digits[4..6])?,
            byte_at(&digits[6..8])?,
        ),
        _ => return None,
    };
    Some(Srgb {
        r: red / 255.0,
        g: green / 255.0,
        b: blue / 255.0,
        a: alpha / 255.0,
    })
}

/// Split a CSS function's arguments on commas / whitespace / a `/` alpha slash.
fn args(inner: &str) -> Vec<String> {
    inner
        .replace([',', '/'], " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// One numeric argument: a bare number or a percentage of `full`.
fn num(tok: &str, full: f64) -> Option<f64> {
    if let Some(p) = tok.strip_suffix('%') {
        return p.parse::<f64>().ok().map(|v| v / 100.0 * full);
    }
    tok.parse::<f64>().ok()
}

/// `rgb(r g b [/ a])` — channels 0–255 or percentages.
fn parse_rgb_fn(inner: &str) -> Option<Srgb> {
    let a = args(inner);
    if a.len() < 3 {
        return None;
    }
    let r = num(&a[0], 255.0)? / 255.0;
    let g = num(&a[1], 255.0)? / 255.0;
    let b = num(&a[2], 255.0)? / 255.0;
    let alpha = a.get(3).and_then(|t| num(t, 1.0)).unwrap_or(1.0);
    Some(Srgb {
        r: r.clamp(0.0, 1.0),
        g: g.clamp(0.0, 1.0),
        b: b.clamp(0.0, 1.0),
        a: alpha.clamp(0.0, 1.0),
    })
}

/// `oklch(L C H [/ a])` → sRGB (gamut-clipped).
fn parse_oklch_fn(inner: &str) -> Option<Srgb> {
    let a = args(inner);
    if a.len() < 3 {
        return None;
    }
    // `L` may be `0..1` or a percentage; `62%` and `0.62` mean the same thing.
    let l = num(&a[0], 1.0)?;
    let c = num(&a[1], 0.4)?;
    let h = num(&a[2], 360.0)?;
    let alpha = a.get(3).and_then(|t| num(t, 1.0)).unwrap_or(1.0);
    Some(oklch_to_srgb(
        Oklch {
            l,
            c,
            h: h.rem_euclid(360.0),
        },
        alpha.clamp(0.0, 1.0),
    ))
}

/// OKLCH → sRGB (clipped to gamut). Exposed so a checker can round-trip a token.
#[must_use]
pub fn oklch_to_srgb(color: Oklch, alpha: f64) -> Srgb {
    // OKLCH (polar) → OKLab (cartesian).
    let green_red = color.c * color.h.to_radians().cos();
    let blue_yellow = color.c * color.h.to_radians().sin();
    // OKLab → LMS'.
    let root_l = color.l + 0.396_337_777_4 * green_red + 0.215_803_757_3 * blue_yellow;
    let root_m = color.l - 0.105_561_345_8 * green_red - 0.063_854_172_8 * blue_yellow;
    let root_s = color.l - 0.089_484_177_5 * green_red - 1.291_485_548_0 * blue_yellow;
    let (cone_l, cone_m, cone_s) = (root_l.powi(3), root_m.powi(3), root_s.powi(3));
    // LMS → linear sRGB.
    let red = 4.076_741_662_1 * cone_l - 3.307_711_591_3 * cone_m + 0.230_969_929_2 * cone_s;
    let green = -1.268_438_004_6 * cone_l + 2.609_757_401_1 * cone_m - 0.341_319_396_5 * cone_s;
    let blue = -0.004_196_086_3 * cone_l - 0.703_418_614_7 * cone_m + 1.707_614_701_0 * cone_s;
    Srgb {
        r: linear_to_srgb(red),
        g: linear_to_srgb(green),
        b: linear_to_srgb(blue),
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Srgb {
        parse_color(s).expect("hex parses")
    }

    #[test]
    fn parses_every_syntax_a_real_token_file_ships() {
        assert!(hex("#fff").near(hex("#ffffff"), 0));
        assert!(hex("#1D4ED8").near(hex("#1d4ed8"), 0));
        assert!(parse_color("#1d4ed8ff").is_some());
        assert!(parse_color("rgb(29, 78, 216)")
            .unwrap()
            .near(hex("#1d4ed8"), 1));
        assert!(parse_color("rgb(29 78 216 / 0.5)")
            .unwrap()
            .near(hex("#1d4ed8"), 1));
        assert!(parse_color("rgba(29,78,216,0.5)")
            .unwrap()
            .near(hex("#1d4ed8"), 1));
        // Unparseable / indirect values are a neutral skip, never an error.
        assert!(parse_color("var(--color-primary)").is_none());
        assert!(parse_color("currentColor").is_none());
        assert!(parse_color("linear-gradient(red, blue)").is_none());
        assert!(parse_color("").is_none());
    }

    #[test]
    fn oklch_round_trips_within_tolerance() {
        // The pack palettes ship OKLCH; a round-trip must land back on the hex
        // they were derived from (within a rounding unit or two).
        for (ok, h) in [
            ("oklch(48.8% 0.217 264.4)", "#1d4ed8"),
            ("oklch(98.5% 0 0)", "#fafafa"),
            ("oklch(21.0% 0.006 285.9)", "#18181b"),
            ("oklch(0.818 0.137 180.7)", "#36e0c8"),
        ] {
            let a = parse_color(ok).expect("oklch parses");
            assert!(
                a.near(hex(h), 3),
                "{ok} should round-trip to {h}, got {a:?}"
            );
        }
    }

    #[test]
    fn contrast_matches_the_wcag_reference_values() {
        // Reference points: black-on-white is the 21:1 ceiling; a same-color pair
        // is the 1:1 floor.
        assert!((contrast_ratio(hex("#000"), hex("#fff")) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(hex("#777"), hex("#777")) - 1.0).abs() < 0.001);
        // A real pair from the modern-minimal pack clears AA body text.
        assert!(contrast_ratio(hex("#18181b"), hex("#fafafa")) > 4.5);
        // A classic grey-on-grey failure does not.
        assert!(contrast_ratio(hex("#999999"), hex("#ffffff")) < 4.5);
    }

    #[test]
    fn ai_purple_band_catches_the_tells_and_spares_real_blues() {
        for tell in [
            "#6366f1", "#4f46e5", "#8b5cf6", "#7c3aed", "#a855f7", "#9333ea", "#818cf8", "#a78bfa",
            "#bc8cff", "#764ba2",
        ] {
            assert!(
                is_ai_purple(hex(tell)),
                "{tell} must be in the AI-purple band"
            );
        }
        for ok in [
            "#2563eb", "#5e8bff", "#0969da", "#3b82f6", "#0000ff", "#1a1aff", "#0ea5e9", "#36e0c8",
            "#c8a96a", "#e6ff00", "#ff6b35", "#c0392b",
        ] {
            assert!(
                !is_ai_purple(hex(ok)),
                "{ok} must NOT be flagged as AI-purple"
            );
        }
        // A near-gray lavender tint (below the chroma floor) is a surface, not a
        // brand hue — not flagged.
        assert!(!is_ai_purple(hex("#f4f3f7")));
    }

    #[test]
    fn never_panics_on_junk() {
        for junk in [
            "#",
            "#zz",
            "#12345",
            "rgb()",
            "oklch(",
            "oklch(a b c)",
            "   ",
            "#你好世",
        ] {
            let _ = parse_color(junk);
        }
    }
}
