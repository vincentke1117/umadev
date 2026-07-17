//! Deterministic design-lint REGISTRY (UD-CODE-002 / UD-CODE-007 family).
//!
//! A dependency-free, fail-open scanner for the design defects that a borrowed
//! brain ships under pressure. Every rule is:
//!
//! - **deterministic** — a pure string/number scan over comment-stripped source.
//!   No DOM, no regex engine, no network. It never panics and never errors.
//! - **numeric** — each rule carries a THRESHOLD, not a vibe (`≥3 font sizes
//!   whose max/min < 2.0`, `border-radius ≥ 24px`, `line-height < 1.3`), so a
//!   finding is falsifiable and a passing file is genuinely passing.
//! - **observable** — each rule states the TELL a reviewer can look for.
//! - **positive** — each rule states the TARGET to move to. A bare prohibition
//!   tells the brain what not to type and leaves it to invent the replacement;
//!   naming the target is what actually changes the output.
//! - **register-scoped** — see [`Register`]. The single most expensive design
//!   mistake is applying MARKETING-surface rules to a dashboard: "no system
//!   fonts", "3x type jumps", "extreme weights", "one orchestrated page-load
//!   reveal" are all CORRECT for a landing page and all WRONG for an admin
//!   console. A rule declares the register it belongs to; an UNKNOWN register
//!   runs everything (fail-open to the historical behaviour).
//!
//! Severity is two-tier: [`DesignSeverity::Hard`] is a small P0 subset the
//! deterministic floor may BLOCK on; [`DesignSeverity::Soft`] is advisory and
//! folds into the rework directive as a quality signal. Callers decide — the
//! governance contract stays fail-open (an exceptional input yields an empty
//! finding list, never an error).

use crate::color::{is_ai_purple, parse_color, Srgb};
use crate::tokenizer::Tokenized;

/// File extensions this detector scans — UI code AND stylesheets (design
/// tells live in both `.tsx` and `.css`).
const DESIGN_EXTS: &[&str] = &[
    "tsx", "ts", "jsx", "js", "vue", "svelte", "astro", "css", "scss", "sass", "less", "html",
];

/// Lowercased file extension after the last `.` (empty if none).
fn ext_of(file_path: &str) -> String {
    file_path
        .rsplit('.')
        .next()
        .filter(|e| *e != file_path)
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// How strongly a design finding should be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignSeverity {
    /// P0 — a near-certain defect the deterministic floor may BLOCK on.
    Hard,
    /// Advisory — a quality signal that folds into the rework directive.
    Soft,
}

/// Which DESIGN REGISTER a surface belongs to — the axis that decides whether a
/// visual rule is right or actively harmful.
///
/// - [`Register::Brand`] — landing / marketing / campaign / portfolio. **Design
///   IS the product**: a distinctive display face, dramatic type jumps, and one
///   orchestrated entrance are the job.
/// - [`Register::Product`] — app / dashboard / admin / settings / devtool.
///   **Design SERVES the task**: a familiar neutral system face is CORRECT, the
///   type scale is a fixed 1.125–1.2 rem ratio, there is NO page-load
///   choreography, restraint is the floor, and density is a virtue.
/// - [`Register::Unknown`] — we could not tell. Runs EVERY rule, exactly as the
///   scanner behaved before registers existed (fail-open: an unknown register
///   never silently disables a check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Register {
    /// Landing / marketing / campaign / portfolio — design is the product.
    Brand,
    /// App / dashboard / admin / devtool — design serves the task.
    Product,
    /// Undetermined — run every rule (the historical behaviour).
    #[default]
    Unknown,
}

impl Register {
    /// Stable lowercase id for events / logs / plan JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brand => "brand",
            Self::Product => "product",
            Self::Unknown => "unknown",
        }
    }

    /// Tolerant parse of a register named in a doc / plan / frontmatter.
    /// Anything unrecognised is [`Register::Unknown`] (fail-open).
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let l = s.trim().to_ascii_lowercase();
        if l.contains("brand")
            || l.contains("landing")
            || l.contains("marketing")
            || l.contains("campaign")
            || l.contains("portfolio")
        {
            // A doc that names BOTH is a mixed surface; the stricter (product)
            // reading is the safe one — it never demands marketing flourish.
            if l.contains("product")
                || l.contains("dashboard")
                || l.contains("admin")
                || l.contains("app")
            {
                return Self::Unknown;
            }
            return Self::Brand;
        }
        if l.contains("product")
            || l.contains("dashboard")
            || l.contains("admin")
            || l.contains("console")
            || l.contains("settings")
            || l.contains("devtool")
            || l.contains("tool")
            || l.contains("app")
        {
            return Self::Product;
        }
        Self::Unknown
    }
}

/// The register(s) a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// Holds in every register — token discipline, contrast, real content.
    Any,
    /// Only a marketing surface. Firing this on a dashboard makes it worse.
    BrandOnly,
    /// Only a product surface. Firing this on a landing page makes it timid.
    ProductOnly,
}

impl RuleScope {
    /// Whether a rule in this scope should run for `register`. An UNKNOWN
    /// register runs everything — the fail-open default.
    #[must_use]
    pub const fn applies(self, register: Register) -> bool {
        matches!(
            (self, register),
            (Self::Any, _)
                | (_, Register::Unknown)
                | (Self::BrandOnly, Register::Brand)
                | (Self::ProductOnly, Register::Product)
        )
    }
}

/// One rule in the design-lint registry: a stable id, a severity tier, the
/// register it belongs to, the OBSERVABLE TELL (with its numeric threshold),
/// and the POSITIVE TARGET to move to.
#[derive(Debug, Clone, Copy)]
pub struct DesignRule {
    /// Stable rule id (e.g. `ai-color-palette`). Never renamed.
    pub id: &'static str,
    /// P0 (`Hard`, may block) vs advisory (`Soft`).
    pub severity: DesignSeverity,
    /// The register this rule belongs to.
    pub scope: RuleScope,
    /// What a reviewer can OBSERVE — the numeric threshold that fired.
    pub tell: &'static str,
    /// What to do INSTEAD. A prohibition without a target backfires.
    pub redirect: &'static str,
}

/// The design-lint registry. Every rule is deterministic, numerically
/// thresholded, register-scoped, and carries both its tell and its target.
pub const DESIGN_RULES: &[DesignRule] = &[
    DesignRule {
        id: "ai-color-palette",
        severity: DesignSeverity::Hard,
        scope: RuleScope::Any,
        tell: "a color in the AI indigo/violet band (OKLCH hue 270-320 at chroma >= 0.09, \
               lightness 0.35-0.85) is used as a brand color or a gradient stop",
        redirect: "commit to a hue this product OWNS (the design pack's --color-primary), and \
                   measure its on- foreground against WCAG",
    },
    DesignRule {
        id: "gradient-text",
        severity: DesignSeverity::Hard,
        scope: RuleScope::Any,
        tell: "background-clip: text (or bg-clip-text) over a gradient",
        redirect: "set the heading to a solid token color; let size and weight carry the emphasis",
    },
    DesignRule {
        id: "broken-image",
        severity: DesignSeverity::Hard,
        scope: RuleScope::Any,
        tell: "an <img> with an empty, missing, or `#` src",
        redirect: "ship the real asset, or render the empty state — never a broken box",
    },
    DesignRule {
        id: "flat-type-hierarchy",
        severity: DesignSeverity::Soft,
        scope: RuleScope::BrandOnly,
        tell: ">= 3 distinct font sizes whose max/min ratio is < 2.0 — nothing leads the page",
        redirect: "on a marketing surface let the display step be dramatic (>= 2.5x the body \
                   size); hierarchy is what a visitor reads first",
    },
    DesignRule {
        id: "monotonous-spacing",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: ">= 10 spacing values but <= 3 unique ones, and one value is > 60% of them — \
               everything is equidistant, so nothing is grouped",
        redirect: "use the 4pt scale to GROUP: tight inside a group (8-12px), open between \
                   groups (24-48px)",
    },
    DesignRule {
        id: "bounce-easing",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a cubic-bezier whose y control point falls outside [-0.1, 1.1] (overshoot), or \
               an animate-bounce utility",
        redirect: "use a crafted ease-out, e.g. cubic-bezier(0.16, 1, 0.3, 1)",
    },
    DesignRule {
        id: "layout-transition",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a transition/animation property naming width, height, padding, or margin — each \
               frame forces a layout pass",
        redirect: "animate transform and opacity only (they run on the compositor)",
    },
    DesignRule {
        id: "over-round",
        severity: DesignSeverity::Soft,
        scope: RuleScope::ProductOnly,
        tell: "border-radius >= 24px on a card / section / input / button",
        redirect: "6-12px on product chrome — the eye should land on the content, not the \
                   container",
    },
    DesignRule {
        id: "hairline-plus-wide-shadow",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "the same rule declares a 1px border AND a box-shadow with blur >= 16px — two \
               elevation languages fighting",
        redirect: "pick ONE elevation language: border-led (1px, no shadow) or shadow-led (no \
                   border)",
    },
    DesignRule {
        id: "dark-glow",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a dark surface (luminance < 0.2) with a saturated colored box-shadow, blur > 4px",
        redirect: "on dark, raise elevation with a LIGHTER surface token, not a neon glow",
    },
    DesignRule {
        id: "crushed-tracking",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "letter-spacing < -0.04em — the letters collide",
        redirect: "keep display tracking in -0.01em..-0.02em; -0.04em is the hard floor",
    },
    DesignRule {
        id: "tiny-text",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a font-size below 12px",
        redirect: "12px is the floor for any readable label; body text is 14-16px",
    },
    DesignRule {
        id: "tight-leading",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "line-height < 1.3 on body-scale text (font-size < 24px in the same rule)",
        redirect: "body leading is 1.5-1.7; only display type goes below 1.3",
    },
    DesignRule {
        id: "em-dash-overuse",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: ">= 5 em-dashes in the body copy of one file",
        redirect: "prefer periods and commas; an em-dash is a spice, not a staple",
    },
    DesignRule {
        id: "marketing-buzzword",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: ">= 2 distinct generic marketing phrases (streamline your / supercharge / \
               industry-leading / ...)",
        redirect: "name the concrete benefit this product delivers, in the user's own words",
    },
    DesignRule {
        id: "numbered-section-markers",
        severity: DesignSeverity::Soft,
        scope: RuleScope::BrandOnly,
        tell: ">= 3 sequential 01 / 02 / 03 section markers in body text",
        redirect: "give each section a real, specific label; the numbering adds no information",
    },
    DesignRule {
        id: "zindex-magic",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a z-index >= 999",
        redirect:
            "declare a semantic layer scale (--z-dropdown / --z-modal / --z-toast) and use it",
    },
    DesignRule {
        id: "invented-metrics",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "an unverifiable stat in copy (trusted by N+ / N% uptime / Nx faster)",
        redirect: "cite a real, sourced number, or drop the claim",
    },
    DesignRule {
        id: "placeholder-name",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a placeholder identity in shipped copy (Jane Doe / Acme Corp / ...)",
        redirect: "use realistic, product-specific names",
    },
    DesignRule {
        id: "overused-font",
        severity: DesignSeverity::Soft,
        scope: RuleScope::BrandOnly,
        tell: "a generic default (Inter / Roboto / Arial / ...) is the LEAD family",
        redirect: "on a marketing surface pick one distinctive display face (the default may stay \
                   in the fallback stack). In the PRODUCT register a familiar neutral face is the \
                   CORRECT choice and this rule does not apply",
    },
    DesignRule {
        id: "heavy-display-weight",
        severity: DesignSeverity::Soft,
        scope: RuleScope::ProductOnly,
        tell: "font-weight 800/900 (or 700 alongside a display-scale size) on a product surface",
        redirect: "product type lives in 400/500/600; let size, color, and spacing carry \
                   hierarchy. (A brand surface MAY use extreme weights deliberately)",
    },
    DesignRule {
        id: "cream-band",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "a very light warm off-white surface (min channel >= 209, r>=g>=b, r-b in 6..48)",
        redirect: "use the chosen pack's surface token — a near-white carrying the brand's own \
                   slight temperature",
    },
    DesignRule {
        id: "pure-bw-surface",
        severity: DesignSeverity::Soft,
        scope: RuleScope::Any,
        tell: "pure #000 / #fff as a background fill",
        redirect: "use an off-black / off-white with a faint brand tint (e.g. #0a0a0b / #fafaf9)",
    },
];

/// Look up a rule by id.
#[must_use]
pub fn rule(id: &str) -> Option<&'static DesignRule> {
    DESIGN_RULES.iter().find(|r| r.id == id)
}

/// One design-quality finding.
#[derive(Debug, Clone)]
pub struct DesignFinding {
    /// Stable rule id (e.g. `ai-color-palette`).
    pub rule: &'static str,
    /// How strongly to treat it.
    pub severity: DesignSeverity,
    /// The full message: the concrete evidence, the observable tell, and the
    /// positive target — in that order.
    pub note: String,
}

impl DesignFinding {
    /// Whether the deterministic floor may BLOCK on this finding (the P0 tier).
    #[must_use]
    pub fn blocking(&self) -> bool {
        self.severity == DesignSeverity::Hard
    }
}

/// Build a finding for `id` from concrete `evidence`. Carries BOTH the tell and
/// the target, because a bare prohibition backfires — the brain needs to know
/// what to type instead, not only what to avoid.
fn finding(id: &'static str, evidence: impl AsRef<str>) -> Option<DesignFinding> {
    let r = rule(id)?;
    Some(DesignFinding {
        rule: r.id,
        severity: r.severity,
        note: format!(
            "{} — tell: {}. Do this instead: {}",
            evidence.as_ref(),
            r.tell,
            r.redirect
        ),
    })
}

/// Scan one UI source file with the register UNKNOWN — every rule runs. This is
/// the historical entry point and is preserved byte-for-byte in behaviour for
/// callers that do not know the register.
#[must_use]
pub fn scan_design_quality(file_path: &str, content: &str) -> Vec<DesignFinding> {
    scan_design_rules(file_path, content, Register::Unknown)
}

/// What the USER's own words already settled, which no default may overrule.
///
/// Today it carries one decision, and that one decision is load-bearing: the
/// `ai-color-palette` rule is a DEFAULT-REJECT of the indigo/violet band, not a
/// censor. A user who says "our brand color is violet `#7c3aed`" has ANSWERED the
/// question the rule exists to ask. Two checks read the same band — the token-level
/// banned-hue rule and this source-level lint — and if only ONE of them honours the
/// user, the two disagree: the tokens are accepted, the component that uses them is
/// blocked, the fix for one is the violation of the other, and the build cannot
/// converge. So the permission travels WITH the scan.
///
/// The permission is **decided once per run, by the brain**, and read back from the
/// persisted [`crate::rules::ProjectContext`]. This crate never derives it: a governor
/// owns no model, and "did the user authorize this hue?" is an intent question, not a
/// lexical one. `Default` is the strict, armed posture — permission withheld.
#[derive(Debug, Clone, Copy, Default)]
pub struct DesignIntent {
    /// The requirement explicitly authorized a purple/violet/indigo brand. The
    /// `ai-color-palette` rule stands down — exactly as the token-level banned-hue
    /// rule does on the same condition.
    ///
    /// Sourced from [`crate::rules::ProjectContext::purple_allowed`], which the run
    /// writes once from a single structured brain consult. Defaults to `false`.
    pub purple_allowed: bool,
}

/// Does the requirement MENTION a color in the flagged indigo/violet band at all — by name
/// (`purple` / `violet` / `indigo` / `紫` / …) or by value (`#7c3aed`, `rgb(124,58,237)`,
/// `oklch(…)`)?
///
/// **This is a pre-filter, not a judge. It can only ARM the rule, never stand it down.**
///
/// The question the anti-slop rule actually needs answered is *"did the user AUTHORIZE this
/// color family?"* — and that is an INTENT question, of exactly the same class as "is this
/// turn chat, an edit, or a build". UmaDev answers intent questions by asking the borrowed
/// brain (see [`crate::rules::ProjectContext::purple_allowed`], written once per run from a
/// single structured consult), never by lexing the user's sentence. It used to lex it, and
/// the lexer could not converge: six review rounds, six leaks, each one shipping the
/// canonical AI hero gradient into a repo whose brief said "we are banning purple", "紫色被
/// 客户否决了", "紫色主题要删掉". A prohibition can be phrased in unboundedly many ways; a
/// growing word list is the wrong shape of answer.
///
/// What survives is the ONE thing a word list *can* decide soundly, because it decides it in
/// the ARMED direction: a requirement that never names the hue at all — the overwhelmingly
/// common case — needs no consult, and the rule simply stays on. `false` here means "nobody
/// could possibly have authorized it, don't even ask"; `true` means only "ask the brain".
/// Neither answer can grant permission, so no addition to `PURPLE_WORDS` can ever leak one.
///
/// Fail-open by shape: an unparseable literal, a proper noun, a prohibition, a quoted code
/// fence — every one of them merely returns `true` and costs one cheap consult, which then
/// answers correctly. Never errors, never panics.
#[must_use]
pub fn requirement_mentions_flagged_color(requirement: &str) -> bool {
    // ASCII-only lowercase: byte-length-preserving, so CJK is untouched.
    let lower: String = requirement
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    !purple_word_spans(&lower).is_empty() || !color_literal_spans(&lower, is_ai_purple).is_empty()
}

/// Latin color words that name the flagged band (matched whole-word).
const PURPLE_WORDS: &[&str] = &["purple", "violet", "indigo", "lavender", "mauve"];
/// CJK color terms that name the flagged band (no word boundaries in CJK — substring).
const PURPLE_CJK: &[&str] = &["紫", "靛"];

/// Byte spans of every purple/violet/indigo mention BY NAME in `lower`.
fn purple_word_spans(lower: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for word in PURPLE_WORDS {
        let mut from = 0;
        while let Some(idx) = lower[from..].find(word) {
            let start = from + idx;
            let end = start + word.len();
            if is_word_boundary(lower, start, end) {
                out.push((start, end));
            }
            from = end;
        }
    }
    for term in PURPLE_CJK {
        let mut from = 0;
        while let Some(idx) = lower[from..].find(term) {
            let start = from + idx;
            let end = start + term.len();
            out.push((start, end));
            from = end;
        }
    }
    out.sort_unstable();
    out
}

/// Is `lower[start..end]` a standalone word (not a fragment of a longer one)? `indigo`
/// inside `indigos` / `indigoairlines` is NOT the color word.
fn is_word_boundary(lower: &str, start: usize, end: usize) -> bool {
    let before_ok = lower[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_ascii_alphanumeric());
    let after_ok = lower[end..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphanumeric());
    before_ok && after_ok
}

/// Scan one UI source file against the registry, honouring `register`.
///
/// The user's own words are NOT consulted on this path, so the `ai-color-palette`
/// default-reject applies. A caller that HAS the requirement must use
/// [`scan_design_rules_with`] so a user who explicitly asked for a purple brand is not
/// blocked by the rule that exists to stop an UNCHOSEN purple.
///
/// Returns an empty list for a non-UI file or clean input. Fail-open: never
/// errors, never panics; an unparseable fragment simply yields no finding.
#[must_use]
pub fn scan_design_rules(file_path: &str, content: &str, register: Register) -> Vec<DesignFinding> {
    scan_design_rules_with(file_path, content, register, DesignIntent::default())
}

/// [`scan_design_rules`], with what the user already decided (see [`DesignIntent`]).
///
/// Returns an empty list for a non-UI file or clean input. Fail-open: never
/// errors, never panics; an unparseable fragment simply yields no finding.
#[must_use]
#[allow(clippy::too_many_lines)] // a flat checklist of independent detectors
pub fn scan_design_rules_with(
    file_path: &str,
    content: &str,
    register: Register,
    intent: DesignIntent,
) -> Vec<DesignFinding> {
    let ext = ext_of(file_path);
    if !DESIGN_EXTS.contains(&ext.as_str()) {
        return Vec::new();
    }
    // Scan code + strings + JSX text, skipping comments — the same view the
    // emoji/color rules use, so a comment can't trip or hide a finding.
    let tz = Tokenized::new(content);
    let body = tz.without_comments(content);
    let lower = body.to_ascii_lowercase();

    let mut out: Vec<DesignFinding> = Vec::new();
    let mut push = |id: &'static str, ev: String| {
        if rule(id).is_some_and(|r| r.scope.applies(register)) {
            if let Some(f) = finding(id, ev) {
                out.push(f);
            }
        }
    };

    // ── P0 ────────────────────────────────────────────────────────────────
    // The indigo/violet band is a DEFAULT-reject, never a censor: a user who asked for
    // a purple brand gets purple, here exactly as at the token level. Without this the
    // two checks contradict each other and the build cannot converge.
    if !intent.purple_allowed {
        if let Some(hex) = ai_purple_literal(&lower) {
            push(
                "ai-color-palette",
                format!("AI indigo/violet `{hex}` in UI source"),
            );
        }
    }
    if has_gradient_text(&lower) {
        push("gradient-text", "gradient headline text".to_string());
    }
    if let Some(tag) = broken_image(&lower) {
        push("broken-image", format!("`{tag}`"));
    }

    // ── Type ──────────────────────────────────────────────────────────────
    let sizes = font_sizes_px(&lower);
    if sizes.len() >= 3 {
        let (min, max) = min_max(&sizes);
        if min > 0.0 && max / min < 2.0 {
            push(
                "flat-type-hierarchy",
                format!(
                    "{} font sizes spanning only {min:.0}px..{max:.0}px (ratio {:.2})",
                    sizes.len(),
                    max / min
                ),
            );
        }
    }
    if let Some(px) = sizes.iter().copied().find(|s| *s < 12.0 && *s > 0.0) {
        push("tiny-text", format!("font-size {px:.1}px"));
    }
    if let Some(t) = crushed_tracking(&lower) {
        push("crushed-tracking", format!("letter-spacing {t}em"));
    }
    if let Some(lh) = tight_leading(&body) {
        push(
            "tight-leading",
            format!("line-height {lh} on body-scale text"),
        );
    }
    if let Some(w) = heavy_display_weight(&lower) {
        push("heavy-display-weight", format!("font-weight {w}"));
    }
    if let Some(font) = overused_primary_font(&lower) {
        push("overused-font", format!("`{font}` as the lead family"));
    }

    // ── Space / shape / depth ─────────────────────────────────────────────
    if let Some(ev) = monotonous_spacing(&lower) {
        push("monotonous-spacing", ev);
    }
    if let Some(px) = over_round(&lower) {
        push(
            "over-round",
            format!("border-radius {px:.0}px on product chrome"),
        );
    }
    if hairline_plus_wide_shadow(&body) {
        push(
            "hairline-plus-wide-shadow",
            "a 1px border and a wide box-shadow on the same rule".to_string(),
        );
    }
    if dark_glow(&body) {
        push(
            "dark-glow",
            "a colored glow shadow on a dark surface".to_string(),
        );
    }
    if let Some(z) = zindex_magic(&lower) {
        push("zindex-magic", format!("z-index: {z}"));
    }

    // ── Motion ────────────────────────────────────────────────────────────
    if has_overshoot_easing(&lower) || lower.contains("animate-bounce") {
        push(
            "bounce-easing",
            "an overshooting / bouncing easing".to_string(),
        );
    }
    if let Some(prop) = layout_transition(&lower) {
        push("layout-transition", format!("transition on `{prop}`"));
    }

    // ── Color surfaces ────────────────────────────────────────────────────
    if let Some(hex) = cream_band_hex(&lower) {
        push("cream-band", format!("AI cream/beige surface `{hex}`"));
    }
    if let Some(hex) = pure_bw_surface(&lower) {
        push("pure-bw-surface", format!("pure `{hex}` as a surface fill"));
    }

    // ── Copy ──────────────────────────────────────────────────────────────
    let hits: Vec<&str> = BUZZWORDS
        .iter()
        .filter(|b| lower.contains(**b))
        .copied()
        .collect();
    if hits.len() >= 2 {
        push("marketing-buzzword", format!("`{}`", hits.join("`, `")));
    }
    if let Some(metric) = invented_metric(&lower) {
        push("invented-metrics", format!("\"{metric}\""));
    }
    if let Some(name) = PLACEHOLDER_NAMES.iter().find(|n| lower.contains(**n)) {
        push("placeholder-name", format!("\"{name}\""));
    }
    let em_dashes = body.matches('\u{2014}').count();
    if em_dashes >= 5 {
        push("em-dash-overuse", format!("{em_dashes} em-dashes"));
    }
    if numbered_section_markers(&lower) {
        push(
            "numbered-section-markers",
            "sequential 01 / 02 / 03 markers".to_string(),
        );
    }

    out
}

// ===================================================================
// detectors — each one pure, bounded, and panic-free on any input
// ===================================================================

/// Marketing buzzwords that signal generic, non-product-specific copy.
const BUZZWORDS: &[&str] = &[
    "streamline your",
    "empower your",
    "supercharge",
    "unleash",
    "leverage the power",
    "best-in-class",
    "industry-leading",
    "enterprise-grade",
    "next-generation",
    "cutting-edge",
    "revolutionize",
    "game-changer",
    "game changer",
    "mission-critical",
    "world-class",
    "seamless experience",
    "future-proof",
];

/// Placeholder identities that must never ship in real copy.
const PLACEHOLDER_NAMES: &[&str] = &[
    "jane doe",
    "john doe",
    "john smith",
    "acme corp",
    "acme inc",
    "acme co",
];

/// Default font families that read as "AI generated" when used as the PRIMARY
/// face ON A MARKETING SURFACE. In the product register they are the CORRECT
/// choice (see [`RuleScope::BrandOnly`] on `overused-font`).
const OVERUSED_FONTS: &[&str] = &[
    "inter",
    "roboto",
    "open sans",
    "lato",
    "montserrat",
    "poppins",
    "nunito",
    "arial",
    "helvetica",
];

/// The first color LITERAL in source that lands in the AI indigo/violet band.
/// Band-based (see [`crate::color::is_ai_purple`]), so a near-neighbour of a
/// canonical tell is caught too — and real blues are deliberately spared.
pub(crate) fn ai_purple_literal(lower: &str) -> Option<String> {
    let (start, end) = color_literal_spans(lower, is_ai_purple)
        .into_iter()
        .next()?;
    Some(lower[start..end].to_string())
}

/// The first color LITERAL in `lower` that lands in the rose/pink/fuchsia band
/// ([`crate::color::is_ai_pink`]). The counterpart of [`ai_purple_literal`], so the
/// purple→pink gradient rule reads BOTH ends of the tell as a hue band instead of a
/// hand-written hex list (which `#db2777` / `#f43f5e` — two of the commonest pink stops
/// there are — walked straight through).
pub(crate) fn ai_pink_literal(lower: &str) -> Option<String> {
    let (start, end) = color_literal_spans(lower, crate::color::is_ai_pink)
        .into_iter()
        .next()?;
    Some(lower[start..end].to_string())
}

/// Byte spans of every color literal in `lower` whose parsed value satisfies `in_band` —
/// `#rgb` / `#rrggbb` / `#rrggbbaa`, `rgb()` / `rgba()`, `oklch()`.
///
/// Spans (not just text) because the requirement reader needs to know WHERE a literal sits
/// to judge whether the user was asking for that hue or forbidding it.
///
/// Two things this is careful about:
/// - **Hex run length.** Only a run of exactly 3 / 6 / 8 hex digits is a color; `#7c3aed001122`
///   is an id, not a violet, and reading the first six digits of a longer run invents a color
///   the author never wrote.
/// - **Alpha.** `#7c3aed00` is INVISIBLE. Reading it as opaque violet blocks a write over a
///   color that renders nothing at all (a fully-transparent overlay/placeholder is a normal
///   thing to write), so a literal with no opacity carries no verdict.
fn color_literal_spans(lower: &str, in_band: fn(Srgb) -> bool) -> Vec<(usize, usize)> {
    /// Below this alpha the color is not on screen, so it cannot be an aesthetic choice.
    const VISIBLE_ALPHA: f64 = 0.02;
    let mut out: Vec<(usize, usize)> = Vec::new();
    let bytes = lower.as_bytes();

    // Hex literals.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'#' {
            i += 1;
            continue;
        }
        // The FULL hex run after `#` — its length is what says whether this is a color.
        let mut end = i + 1;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        let run = end - (i + 1);
        if matches!(run, 3 | 6 | 8) {
            if let Some(c) = parse_color(&lower[i..end]) {
                if c.a > VISIBLE_ALPHA && in_band(c) {
                    out.push((i, end));
                }
            }
        }
        i = end.max(i + 1);
    }

    // `oklch(...)` / `rgb(...)` / `rgba(...)` literals.
    for marker in ["oklch(", "rgb(", "rgba("] {
        let mut from = 0;
        while let Some(idx) = lower[from..].find(marker) {
            let start = from + idx;
            if let Some(close) = lower[start..].find(')') {
                let end = start + close + 1;
                if let Some(c) = parse_color(&lower[start..end]) {
                    if c.a > VISIBLE_ALPHA && in_band(c) {
                        out.push((start, end));
                    }
                }
                from = start + close;
            } else {
                break;
            }
        }
    }
    out.sort_unstable();
    out
}

/// Gradient text (`background-clip: text` over a gradient) — a hero "gradient
/// headline" tell no genre legitimately ships.
fn has_gradient_text(lower: &str) -> bool {
    (lower.contains("background-clip: text")
        || lower.contains("background-clip:text")
        || lower.contains("-webkit-background-clip: text")
        || lower.contains("-webkit-background-clip:text")
        || lower.contains("bg-clip-text"))
        && lower.contains("gradient")
}

/// An `<img>` with an empty / `#` / missing `src`. Returns a short excerpt.
fn broken_image(lower: &str) -> Option<String> {
    let mut from = 0;
    while let Some(idx) = lower[from..].find("<img") {
        let start = from + idx;
        let end = lower[start..]
            .find('>')
            .map_or_else(|| floor_boundary(&lower[start..], 120), |e| e + 1);
        let tag = &lower[start..start + end];
        let broken = match tag.find("src") {
            // NO literal `src` — but a JSX SPREAD (`<img {...props} />`,
            // `<img {...rest} alt="" />`) carries its `src` in a value we cannot see from
            // here. An absent literal is then evidence that the attributes are COMPUTED,
            // not evidence of a broken image — and this is exactly how a correctly-written
            // image wrapper component is spelled. Blocking on it fails a whole idiom, so a
            // spread means we say nothing. (A tag that spreads AND writes an empty `src`
            // is still judged on the `src` it wrote — see the `Some` arm.)
            None => !tag.contains("{..."),
            Some(s) => {
                let rest = &tag[s + 3..];
                let val: String = rest
                    .trim_start()
                    .trim_start_matches('=')
                    .trim_start()
                    .trim_start_matches(['"', '\'', '{'])
                    .chars()
                    .take_while(|c| !matches!(c, '"' | '\'' | '}' | ' ' | '>'))
                    .collect();
                val.is_empty() || val == "#"
            }
        };
        if broken {
            let excerpt: String = tag.chars().take(60).collect();
            return Some(excerpt);
        }
        from = start + end.max(1);
    }
    None
}

/// Every `font-size` declaration's value, normalized to px (rem/em × 16).
fn font_sizes_px(lower: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(idx) = lower[from..].find("font-size") {
        let start = from + idx + "font-size".len();
        let decl = &lower[start..];
        let decl = decl.trim_start().trim_start_matches(':').trim_start();
        let end = decl
            .find([';', '\n', '}', '{', ',', ')'])
            .unwrap_or_else(|| floor_boundary(decl, 24));
        if let Some(px) = length_px(decl[..end].trim()) {
            if px > 0.0 && px < 400.0 && !out.contains(&px) {
                out.push(px);
            }
        }
        from = start;
        if out.len() > 64 {
            break;
        }
    }
    out
}

/// A CSS length in px (`16px`, `1rem`, `1.25em`) → px. `None` for anything else
/// (a `clamp()`, a `var()`, a percentage) — the fail-open path.
fn length_px(value: &str) -> Option<f64> {
    let v = value.trim();
    let num: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let n: f64 = num.parse().ok()?;
    let unit = v[num.len()..].trim();
    match unit {
        "px" => Some(n),
        "rem" | "em" => Some(n * 16.0),
        _ => None,
    }
}

/// Min and max of a non-empty slice.
fn min_max(v: &[f64]) -> (f64, f64) {
    v.iter()
        .fold((f64::MAX, 0.0_f64), |(lo, hi), x| (lo.min(*x), hi.max(*x)))
}

/// A `letter-spacing` below `-0.04em`.
fn crushed_tracking(lower: &str) -> Option<String> {
    for v in decl_values(lower, "letter-spacing", 24) {
        let t = v.trim();
        if let Some(n) = t
            .strip_suffix("em")
            .and_then(|n| n.trim().parse::<f64>().ok())
        {
            if n < -0.04 {
                return Some(format!("{n}"));
            }
        }
    }
    None
}

/// A `line-height < 1.3` inside a rule block whose `font-size` is body-scale
/// (< 24px). Display type legitimately goes below 1.3, so the size qualifier is
/// what makes this rule safe to run in every register.
fn tight_leading(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    for block in rule_blocks(&lower) {
        let Some(lh) = decl_values(block, "line-height", 16)
            .into_iter()
            .find_map(|v| v.trim().parse::<f64>().ok())
        else {
            continue;
        };
        if lh >= 1.3 {
            continue;
        }
        let body_scale = decl_values(block, "font-size", 24)
            .into_iter()
            .filter_map(|v| length_px(v.trim()))
            .any(|px| px < 24.0);
        if body_scale {
            return Some(format!("{lh}"));
        }
    }
    None
}

/// Detect a heavy display weight: `font-weight: 800` / `900` anywhere, or `700`
/// when a display-scale `font-size` (≥ 32px) appears in source.
fn heavy_display_weight(lower: &str) -> Option<&'static str> {
    let has_display_size = font_sizes_px(lower).iter().any(|px| *px >= 32.0);
    for v in decl_values(lower, "font-weight", 24) {
        let weight = v.split_whitespace().next().unwrap_or("");
        if weight == "900" {
            return Some("900");
        }
        if weight == "800" {
            return Some("800");
        }
        if has_display_size && (weight == "700" || weight == "bold") {
            return Some("700");
        }
    }
    None
}

/// A generic default font used as the LEADING family (the primary face).
fn overused_primary_font(lower: &str) -> Option<&'static str> {
    for marker in [
        "font-family",
        "--font-display",
        "--font-sans",
        "--font-heading",
    ] {
        for v in decl_values(lower, marker, 120) {
            let value = v.trim().trim_start_matches(['"', '\'', ' ']);
            let first = value
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['"', '\'']);
            if let Some(f) = OVERUSED_FONTS.iter().find(|f| first == **f) {
                return Some(f);
            }
        }
    }
    None
}

/// `>= 10` spacing values, `<= 3` unique, one of them `> 60%` of the total —
/// everything equidistant, so nothing is grouped.
fn monotonous_spacing(lower: &str) -> Option<String> {
    let mut vals: Vec<i64> = Vec::new();
    for prop in ["padding", "margin", "gap", "row-gap", "column-gap"] {
        for v in decl_values(lower, prop, 60) {
            for tok in v.split_whitespace() {
                if let Some(px) = length_px(tok) {
                    if px > 0.0 && px < 400.0 {
                        vals.push(px.round() as i64);
                    }
                }
            }
        }
        if vals.len() > 400 {
            break;
        }
    }
    if vals.len() < 10 {
        return None;
    }
    let mut uniq: Vec<i64> = vals.clone();
    uniq.sort_unstable();
    uniq.dedup();
    if uniq.len() > 3 {
        return None;
    }
    let (modal, count) = uniq
        .iter()
        .map(|u| (*u, vals.iter().filter(|v| *v == u).count()))
        .max_by_key(|(_, c)| *c)?;
    #[allow(clippy::cast_precision_loss)]
    let share = count as f64 / vals.len() as f64;
    if share > 0.60 {
        return Some(format!(
            "{} spacing values, {} unique, {modal}px used {:.0}% of the time",
            vals.len(),
            uniq.len(),
            share * 100.0
        ));
    }
    None
}

/// A `border-radius >= 24px` on product chrome (a card / section / input /
/// button / panel / modal selector or class near the declaration).
fn over_round(lower: &str) -> Option<f64> {
    const CHROME: &[&str] = &[
        "card", "section", "input", "button", "btn", "panel", "modal", "dialog", "table", "row",
    ];
    for block in rule_blocks(lower) {
        let is_chrome = CHROME.iter().any(|c| block.contains(c));
        if !is_chrome {
            continue;
        }
        for v in decl_values(block, "border-radius", 40) {
            if let Some(px) = v.split_whitespace().find_map(length_px) {
                if (24.0..1000.0).contains(&px) {
                    return Some(px);
                }
            }
        }
    }
    // Utility-class form: `rounded-3xl` (24px) and larger.
    for cls in ["rounded-3xl", "rounded-[24px]", "rounded-[2rem]"] {
        if lower.contains(cls) && CHROME.iter().any(|c| lower.contains(c)) {
            return Some(24.0);
        }
    }
    None
}

/// A 1px border AND a `box-shadow` with a `>= 16px` blur in the SAME rule.
fn hairline_plus_wide_shadow(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    for block in rule_blocks(&lower) {
        let hairline = decl_values(block, "border", 60)
            .iter()
            .chain(decl_values(block, "border-width", 20).iter())
            .any(|v| v.contains("1px"));
        if !hairline {
            continue;
        }
        if shadow_blurs(block).iter().any(|b| *b >= 16.0) {
            return true;
        }
    }
    false
}

/// A dark surface (`background` luminance < 0.2) with a saturated colored
/// `box-shadow` whose blur is `> 4px` — the "neon glow on black" tell.
fn dark_glow(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    for block in rule_blocks(&lower) {
        let dark = decl_values(block, "background", 80)
            .iter()
            .chain(decl_values(block, "background-color", 60).iter())
            .flat_map(|v| colors_in(v))
            .any(|c| c.a > 0.5 && c.luminance() < 0.2);
        if !dark {
            continue;
        }
        for v in decl_values(block, "box-shadow", 160) {
            let blurs = shadow_blurs_of(&v);
            let colored = colors_in(&v)
                .into_iter()
                .any(|c| c.oklch().c > 0.05 && c.a > 0.15);
            if colored && blurs.iter().any(|b| *b > 4.0) {
                return true;
            }
        }
    }
    false
}

/// A `z-index >= 999`.
fn zindex_magic(lower: &str) -> Option<i64> {
    decl_values(lower, "z-index", 16)
        .into_iter()
        .filter_map(|v| v.trim().parse::<i64>().ok())
        .find(|z| *z >= 999)
}

/// A `transition` / `animation` naming a LAYOUT property.
fn layout_transition(lower: &str) -> Option<&'static str> {
    const LAYOUT: &[&str] = &["width", "height", "padding", "margin"];
    for prop in ["transition", "transition-property", "animation"] {
        for v in decl_values(lower, prop, 120) {
            for l in LAYOUT {
                // Match as a whole word so `max-width` in a media query and
                // `transition: all` never trip it.
                if v.split([' ', ',']).any(|t| t.trim() == *l) {
                    return Some(l);
                }
            }
        }
    }
    // Tailwind utility form.
    for (cls, prop) in [
        ("transition-\\[width\\]", "width"),
        ("transition-\\[height\\]", "height"),
    ] {
        if lower.contains(cls) {
            return Some(prop);
        }
    }
    None
}

/// `>= 3` sequential `01` / `02` / `03` markers in body text.
fn numbered_section_markers(lower: &str) -> bool {
    let mut hits = 0;
    for n in ["01", "02", "03", "04", "05"] {
        // Standalone token: not part of a longer number / hex / date.
        let mut from = 0;
        let mut found = false;
        while let Some(idx) = lower[from..].find(n) {
            let at = from + idx;
            let before = lower[..at].chars().last();
            let after = lower[at + 2..].chars().next();
            let boundary = |c: Option<char>| {
                c.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_')
            };
            if boundary(before) && boundary(after) {
                found = true;
                break;
            }
            from = at + 2;
        }
        if found {
            hits += 1;
        }
    }
    hits >= 3
}

/// Detect an overshooting `cubic-bezier(...)`: y1 or y2 outside `[-0.1, 1.1]`.
fn has_overshoot_easing(lower: &str) -> bool {
    let mut from = 0;
    while let Some(idx) = lower[from..].find("cubic-bezier(") {
        let start = from + idx + "cubic-bezier(".len();
        let Some(close_rel) = lower[start..].find(')') else {
            return false;
        };
        let inner = &lower[start..start + close_rel];
        let nums: Vec<f64> = inner
            .split(',')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .collect();
        if nums.len() == 4 && (nums[1] > 1.1 || nums[1] < -0.1 || nums[3] > 1.1 || nums[3] < -0.1) {
            return true;
        }
        from = start + close_rel;
    }
    false
}

/// Find a 6-digit hex in the AI "cream/beige" band: very light, warm,
/// `min(r,g,b) >= 209`, `r >= g >= b`, warmth `(r-b) in 6..=48`.
fn cream_band_hex(lower: &str) -> Option<String> {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 7 <= bytes.len() {
            if let Some(hex) = lower.get(i + 1..i + 7) {
                if hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                    let warmth = i32::from(r) - i32::from(b);
                    if r.min(g).min(b) >= 209 && r >= g && g >= b && (6..=48).contains(&warmth) {
                        return Some(format!("#{hex}"));
                    }
                }
            }
            i += 7;
        } else {
            i += 1;
        }
    }
    None
}

/// Pure `#000` / `#fff` used as a `background` / `background-color` value.
fn pure_bw_surface(lower: &str) -> Option<String> {
    for marker in ["background-color", "background"] {
        for value in decl_values(lower, marker, 120) {
            for pure in ["#000000", "#ffffff", "#000", "#fff"] {
                if let Some(p) = value.find(pure) {
                    let after = value[p + pure.len()..].chars().next();
                    if after.is_none_or(|c| !c.is_ascii_hexdigit()) {
                        return Some(pure.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Detect an invented marketing metric. Returns the matched fragment.
fn invented_metric(lower: &str) -> Option<String> {
    if let Some(idx) = lower.find("trusted by ") {
        let tail = &lower[idx + "trusted by ".len()..];
        if tail.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let frag: String = lower[idx..].chars().take(24).collect();
            return Some(frag.trim().to_string());
        }
    }
    for unit in ["x faster", "% uptime", "% faster", "x more"] {
        if let Some(idx) = lower.find(unit) {
            let before = lower[..idx].trim_end();
            if before.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                let from = floor_boundary(lower, idx.saturating_sub(8));
                let frag: String = lower[from..].chars().take(20).collect();
                return Some(frag.trim().to_string());
            }
        }
    }
    None
}

// ── shared CSS-ish scanning primitives ────────────────────────────────────

/// Every value declared for `prop` in `lower` (`prop: <value>;`). Bounded per
/// declaration by `cap` chars so a runaway value cannot blow the scan.
/// Whole-property match: `border` does NOT also pick up `border-radius`.
fn decl_values(lower: &str, prop: &str, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(idx) = lower[from..].find(prop) {
        let at = from + idx;
        let after_prop = at + prop.len();
        // Left boundary: not part of a longer identifier (`--font-size` IS a
        // legitimate left-extension, so allow `-` before only for `--` custom
        // props, which callers ask for explicitly).
        let before_ok = lower[..at]
            .chars()
            .last()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        // Right boundary: the very next non-space char must be `:`.
        let rest = lower.get(after_prop..).unwrap_or("");
        let trimmed = rest.trim_start();
        if before_ok && trimmed.starts_with(':') {
            let val_start = after_prop + (rest.len() - trimmed.len()) + 1;
            let decl = lower.get(val_start..).unwrap_or("");
            let end = decl
                .find([';', '\n', '}', '{'])
                .unwrap_or_else(|| floor_boundary(decl, cap));
            let end = end.min(floor_boundary(decl, cap.max(8)));
            out.push(decl[..end].trim().to_string());
        }
        from = after_prop.max(at + 1);
        if out.len() > 200 {
            break;
        }
    }
    out
}

/// Split CSS-ish source into `{ ... }` rule bodies (with the selector text that
/// precedes them, so a selector-name check like "card" can look at the block).
/// Bounded: at most 400 blocks. A JSX/TS file yields whatever braces it has —
/// that is fine, the detectors only look for CSS declarations inside.
fn rule_blocks(lower: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = lower.as_bytes();
    let mut open: Option<usize> = None;
    let mut prev = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if open.is_none() {
                    open = Some(prev);
                }
            }
            b'}' => {
                if let Some(s) = open.take() {
                    if let Some(slice) = lower.get(s..i) {
                        out.push(slice);
                    }
                    prev = i + 1;
                    if out.len() >= 400 {
                        return out;
                    }
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(lower);
    }
    out
}

/// Every `box-shadow` blur radius (px) declared in `block`.
fn shadow_blurs(block: &str) -> Vec<f64> {
    decl_values(block, "box-shadow", 160)
        .iter()
        .flat_map(|v| shadow_blurs_of(v))
        .collect()
}

/// The blur radius of each shadow in one `box-shadow` value. CSS order is
/// `<offset-x> <offset-y> <blur> [<spread>] [<color>]`, so the blur is the THIRD
/// length.
///
/// Split on TOP-LEVEL commas only: a `rgba(0, 0, 0, .1)` color carries commas of
/// its own, and splitting inside it would shred the shadow into fragments.
fn shadow_blurs_of(value: &str) -> Vec<f64> {
    let mut out = Vec::new();
    for shadow in split_top_level(value) {
        let lens: Vec<f64> = shadow.split_whitespace().filter_map(shadow_len).collect();
        if lens.len() >= 3 {
            out.push(lens[2].abs());
        }
    }
    out
}

/// A length inside a shadow: `px` / `rem` / `em`, plus a BARE `0` (which CSS
/// allows as a unitless zero — and which is what an offset almost always is).
fn shadow_len(tok: &str) -> Option<f64> {
    let t = tok.trim();
    if t == "0" {
        return Some(0.0);
    }
    length_px(t)
}

/// Split on commas that are NOT inside parentheses.
fn split_top_level(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth <= 0 => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

/// Every color literal inside a CSS value (`#hex`, `rgb(...)`, `oklch(...)`),
/// whether or not the function's own commas carry spaces.
fn colors_in(value: &str) -> Vec<crate::color::Srgb> {
    let mut out = Vec::new();
    for marker in ["rgba(", "rgb(", "oklch("] {
        let mut from = 0;
        while let Some(idx) = value[from..].find(marker) {
            let start = from + idx;
            let Some(close) = value[start..].find(')') else {
                break;
            };
            if let Some(c) = parse_color(&value[start..=start + close]) {
                out.push(c);
            }
            from = start + close;
        }
    }
    for tok in value.split([' ', ',', '(', ')']) {
        if tok.starts_with('#') {
            if let Some(c) = parse_color(tok) {
                out.push(c);
            }
        }
    }
    out
}

/// Largest UTF-8 char boundary at or below `idx` (clamped to `s.len()`), so
/// `&s[..floor_boundary(s, idx)]` never panics on a multibyte char.
fn floor_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

// ===================================================================
// font extraction — used by the UIUX typography-drift check
// ===================================================================

/// Generic/system font families that are universal fallbacks — they may appear
/// in any stack and never need to be declared in a design contract.
const GENERIC_FONTS: &[&str] = &[
    "sans-serif",
    "serif",
    "monospace",
    "system-ui",
    "ui-sans-serif",
    "ui-serif",
    "ui-monospace",
    "ui-rounded",
    "-apple-system",
    "blinkmacsystemfont",
    "segoe ui",
    "arial",
    "helvetica",
    "helvetica neue",
    "roboto",
    "cursive",
    "fantasy",
    "emoji",
    "math",
    "inherit",
    "initial",
    "unset",
    "noto sans",
    "apple color emoji",
    "segoe ui emoji",
];

/// Whether `name` (lowercased, unquoted) is a universal/system fallback font.
#[must_use]
pub fn is_generic_font(name: &str) -> bool {
    GENERIC_FONTS.contains(&name)
}

/// Extract every font-family name referenced in source — the lead and fallback
/// families of each `font-family:` / `--font-*:` declaration, lowercased and
/// unquoted. Used to cross-check generated code against the locked UIUX
/// typography contract (a code font absent from the contract = drift).
#[must_use]
pub fn extract_fonts(content: &str) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "font-family",
        "--font-display",
        "--font-sans",
        "--font-heading",
        "--font-body",
        "--font-mono",
        "--font-serif",
    ];
    let lower = content.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for marker in MARKERS {
        for decl in decl_values(&lower, marker, 160) {
            for fam in decl.split(',') {
                let f = fam.trim().trim_matches(['"', '\'', ' ', '`']);
                if !f.is_empty() && f.len() < 40 && !f.contains("var(") && !f.contains('$') {
                    let f = f.to_string();
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_for(file: &str, content: &str, reg: Register) -> Vec<&'static str> {
        scan_design_rules(file, content, reg)
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }

    fn rules(file: &str, content: &str) -> Vec<&'static str> {
        rules_for(file, content, Register::Unknown)
    }

    #[test]
    fn every_rule_carries_a_numeric_tell_and_a_positive_target() {
        for r in DESIGN_RULES {
            assert!(!r.tell.is_empty(), "{} has no observable tell", r.id);
            assert!(!r.redirect.is_empty(), "{} has no positive target", r.id);
            assert!(
                !r.id.is_empty() && r.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{} is not a stable kebab id",
                r.id
            );
        }
        // A small P0 subset blocks; the rest are advisory.
        let hard = DESIGN_RULES
            .iter()
            .filter(|r| r.severity == DesignSeverity::Hard)
            .count();
        assert!(
            (1..=5).contains(&hard),
            "P0 tier must stay small, got {hard}"
        );
    }

    #[test]
    fn findings_state_both_the_tell_and_the_target() {
        let f = scan_design_quality("src/a.css", "a{color:#6366f1}")
            .into_iter()
            .next()
            .expect("a finding");
        assert!(
            f.note.contains("tell:"),
            "note must carry the tell: {}",
            f.note
        );
        assert!(
            f.note.contains("Do this instead:"),
            "note must carry the positive target: {}",
            f.note
        );
        assert!(f.blocking(), "ai-color-palette is a P0 rule");
    }

    // ── register gating ───────────────────────────────────────────────────

    #[test]
    fn a_neutral_system_font_is_a_defect_on_brand_and_correct_on_product() {
        let css = "body { font-family: Inter, system-ui; }";
        assert!(rules_for("src/a.css", css, Register::Brand).contains(&"overused-font"));
        assert!(
            !rules_for("src/a.css", css, Register::Product).contains(&"overused-font"),
            "a familiar neutral face is CORRECT in the product register"
        );
        // Unknown register = the historical behaviour: the rule still runs.
        assert!(rules("src/a.css", css).contains(&"overused-font"));
    }

    #[test]
    fn a_flat_type_scale_is_a_defect_on_brand_and_correct_on_product() {
        let css = "h1{font-size:20px} h2{font-size:18px} p{font-size:16px}";
        assert!(rules_for("src/a.css", css, Register::Brand).contains(&"flat-type-hierarchy"));
        assert!(
            !rules_for("src/a.css", css, Register::Product).contains(&"flat-type-hierarchy"),
            "a fixed 1.125-1.2 scale is the CORRECT product-register scale"
        );
    }

    #[test]
    fn extreme_weight_is_a_tool_on_brand_and_a_defect_on_product() {
        let css = "h1{font-weight:900}";
        assert!(rules_for("src/a.css", css, Register::Product).contains(&"heavy-display-weight"));
        assert!(!rules_for("src/a.css", css, Register::Brand).contains(&"heavy-display-weight"));
    }

    #[test]
    fn over_round_only_fires_on_product_chrome() {
        let css = ".card{border-radius:32px}";
        assert!(rules_for("src/a.css", css, Register::Product).contains(&"over-round"));
        assert!(!rules_for("src/a.css", css, Register::Brand).contains(&"over-round"));
        // Under the cap it passes in every register.
        assert!(
            !rules_for("src/a.css", ".card{border-radius:12px}", Register::Product)
                .contains(&"over-round")
        );
    }

    // ── P0 tier ───────────────────────────────────────────────────────────

    #[test]
    fn flags_the_ai_purple_band_including_near_neighbours() {
        for tell in ["#6366f1", "#8b5cf6", "#7c3aed", "#bc8cff"] {
            assert!(
                rules("src/Hero.tsx", &format!("const c = '{tell}';"))
                    .contains(&"ai-color-palette"),
                "{tell} must be flagged"
            );
        }
        // An OKLCH-declared purple is caught too (a hex list alone would miss it).
        assert!(rules("src/a.css", "a{color:oklch(60% 0.22 293)}").contains(&"ai-color-palette"));
        // Real blues + the pack accents are NOT flagged.
        for ok in ["#2563eb", "#5e8bff", "#36e0c8", "#c8a96a"] {
            assert!(
                !rules("src/a.css", &format!("a{{color:{ok}}}")).contains(&"ai-color-palette"),
                "{ok} must not be flagged"
            );
        }
    }

    #[test]
    fn flags_gradient_text() {
        let css =
            "h1 { background: linear-gradient(90deg, #f00, #0f0); -webkit-background-clip: text; }";
        assert!(rules("src/a.css", css).contains(&"gradient-text"));
    }

    #[test]
    fn flags_a_broken_image_but_not_a_real_one() {
        assert!(rules("src/a.tsx", "<img src=\"\" alt=\"x\" />").contains(&"broken-image"));
        assert!(rules("src/a.tsx", "<img src=\"#\" alt=\"x\" />").contains(&"broken-image"));
        assert!(rules("src/a.tsx", "<img alt=\"x\" />").contains(&"broken-image"));
        assert!(
            !rules("src/a.tsx", "<img src=\"/hero.png\" alt=\"x\" />").contains(&"broken-image")
        );
    }

    #[test]
    fn a_jsx_spread_is_not_a_broken_image() {
        // `<img {...props} />` carries its `src` in a value the scan cannot see. "No
        // literal src in the tag" is evidence that the attributes are COMPUTED, not
        // evidence of a broken image — and this is how every correctly-written image
        // wrapper component is spelled. Blocking on it fails a whole idiom.
        for src in [
            "<img {...props} />",
            "<img {...rest} alt=\"\" />",
            "export const Img = (props) => <img {...props} className=\"w-full\" />;",
        ] {
            assert!(
                !rules("src/a.tsx", src).contains(&"broken-image"),
                "a spread is not a broken image: {src}"
            );
        }
        // A genuinely empty src is still caught, spread or not.
        assert!(rules("src/a.tsx", "<img {...rest} src=\"\" />").contains(&"broken-image"));
    }

    #[test]
    fn a_requested_purple_brand_stands_the_ai_palette_rule_down() {
        // The indigo/violet band is a DEFAULT-reject, not a censor. The token-level
        // banned-hue rule already stands down when the user asked for purple; if this
        // source-level lint does NOT, the two contradict each other — the tokens are
        // accepted and every component that uses them is blocked — and no edit converges.
        let css = "a{color:#7c3aed}";
        assert!(
            rules("src/a.css", css).contains(&"ai-color-palette"),
            "nobody asked → the default-reject holds"
        );
        let allowed = scan_design_rules_with(
            "src/a.css",
            css,
            Register::Unknown,
            DesignIntent {
                purple_allowed: true,
            },
        );
        assert!(
            !allowed.iter().any(|f| f.rule == "ai-color-palette"),
            "the user asked for violet — the rule that exists to stop an UNCHOSEN purple \
             must not block a chosen one: {:?}",
            allowed.iter().map(|f| f.rule).collect::<Vec<_>>()
        );
        // The permission is scoped to the hue rule alone; everything else still runs.
        let other = scan_design_rules_with(
            "src/a.css",
            "h1{background-clip:text;background:linear-gradient(#7c3aed,#000)}",
            Register::Unknown,
            DesignIntent {
                purple_allowed: true,
            },
        );
        assert!(
            other.iter().any(|f| f.rule == "gradient-text"),
            "a purple brand does not license gradient text: {:?}",
            other.iter().map(|f| f.rule).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_pre_filter_arms_on_every_mention_and_can_never_grant() {
        // What is left of the lexical reader is a PRE-FILTER, and its ONLY sound answer is
        // the ARMED one. It answers "could anyone have authorized this hue in this text?" —
        // never "did they?". So it says YES to a prohibition, to a proper noun, and to a
        // quoted code fence just as readily as to a request: each of those merely costs one
        // cheap brain consult, which is the thing that actually decides.
        //
        // That asymmetry is the whole point. The word list it still carries names COLORS,
        // not intentions — so no addition to it, and no phrasing missing from it, can ever
        // leak a permission. The battery of prohibitions and requests that used to live here
        // now tests the decision itself, over a stub brain, in `umadev_agent::color_permission`.
        for mentions in [
            // Requests.
            "our brand is violet",
            "#7c3aed is our primary",
            "使用紫色作为主色",
            "primary: rgb(124,58,237)",
            "brand = oklch(0.55 0.22 290)",
            // Prohibitions — ALSO true here. The pre-filter does not judge; it defers.
            "We are banning purple from the theme entirely.",
            "紫色被客户否决了,用蓝绿色",
            "紫色主题要删掉,用蓝色",
            "avoid AI-looking templates (purple gradient, emoji icons, default-font-only)",
            // A proper noun / a quoted fence — likewise deferred, not decided.
            "a booking app for IndiGo Airlines",
            "do not use purple.\n```css\n:root{--x:#7c3aed}\n```",
        ] {
            assert!(
                requirement_mentions_flagged_color(mentions),
                "the band is named here — the decision belongs to the brain: {mentions:?}"
            );
        }

        // The one case it may short-circuit: the hue is nowhere in the text, so nobody could
        // have authorized it and there is nothing to ask. The rule simply stays armed.
        for silent in [
            "a clean dashboard for our sales team",
            "primary is #0ea5e9 (sky blue)",
            "brand red #dc2626, nothing else",
            // A brand whose NAME merely contains the letters (`indigo` inside `IndiGo…`) is
            // not a mention — the word boundary still holds.
            "book a flight on indigoairlines.example",
            // An invisible literal is not on screen, so it is not an aesthetic choice.
            "overlay: #7c3aed00",
            "",
        ] {
            assert!(
                !requirement_mentions_flagged_color(silent),
                "nothing names the band — no consult, rule armed: {silent:?}"
            );
        }
    }

    #[test]
    fn the_pre_filter_never_panics_on_multibyte_text() {
        // A governor must never take the host down. Sweep a mixed-script string at every
        // char boundary — the span scan indexes bytes, and CJK text is where a bad slice
        // would panic.
        let mixed = "不要用紫色渐变,brand is violet,主色调用 #7c3aed,IndiGo Airlines,別用紫";
        for end in 0..=mixed.len() {
            if mixed.is_char_boundary(end) {
                let _ = requirement_mentions_flagged_color(&mixed[..end]);
            }
        }
        // …and a hue sitting at the very start / very end of the text.
        assert!(requirement_mentions_flagged_color("紫"));
        assert!(requirement_mentions_flagged_color("purple"));
    }

    #[test]
    fn a_fully_transparent_purple_is_not_a_purple() {
        // `#7c3aed00` is INVISIBLE — alpha 0. Reading its first six digits as opaque violet
        // blocked a write over a color that renders nothing at all. The literal scanner now
        // reads the whole hex run (3/6/8 digits only) and its alpha.
        assert!(ai_purple_literal("background: #7c3aed00;").is_none());
        assert!(ai_purple_literal("outline: rgba(124, 58, 237, 0);").is_none());
        // …while the visible forms still read as the banned hue.
        assert!(ai_purple_literal("background: #7c3aed;").is_some());
        assert!(ai_purple_literal("background: #7c3aedff;").is_some());
        assert!(ai_purple_literal("background: #7c3aed80;").is_some());
        // A longer hex RUN is an id, not a color — six of its digits are not a violet.
        assert!(ai_purple_literal("data-key=\"#7c3aed001122\"").is_none());
    }

    // ── the new advisory rules, each at its threshold ─────────────────────

    #[test]
    fn flags_monotonous_spacing_at_its_threshold() {
        // 12 values, 2 unique, 16px used 83% of the time → monotonous.
        let css = "a{padding:16px}b{padding:16px}c{padding:16px}d{padding:16px}e{padding:16px}\
                   f{padding:16px}g{padding:16px}h{padding:16px}i{padding:16px}j{padding:16px}\
                   k{padding:24px}l{padding:24px}";
        assert!(rules("src/a.css", css).contains(&"monotonous-spacing"));
        // A real 4pt scale with variety passes.
        let ok = "a{padding:4px}b{padding:8px}c{padding:12px}d{padding:16px}e{padding:24px}\
                  f{padding:32px}g{padding:48px}h{padding:8px}i{padding:12px}j{padding:64px}";
        assert!(!rules("src/a.css", ok).contains(&"monotonous-spacing"));
    }

    #[test]
    fn flags_bounce_easing_not_a_crafted_ease() {
        assert!(rules(
            "src/a.css",
            "a{transition:200ms cubic-bezier(0.34,1.56,0.64,1)}"
        )
        .contains(&"bounce-easing"));
        assert!(
            rules("src/a.tsx", "<div className=\"animate-bounce\" />").contains(&"bounce-easing")
        );
        assert!(!rules(
            "src/a.css",
            "a{transition:200ms cubic-bezier(0.16,1,0.3,1)}"
        )
        .contains(&"bounce-easing"));
    }

    #[test]
    fn flags_a_layout_transition_not_a_compositor_one() {
        assert!(rules("src/a.css", "a{transition:width 200ms}").contains(&"layout-transition"));
        assert!(rules("src/a.css", "a{transition:height 1s, opacity 1s}")
            .contains(&"layout-transition"));
        assert!(
            !rules("src/a.css", "a{transition:transform 200ms, opacity 200ms}")
                .contains(&"layout-transition")
        );
        // `max-width` in a media query must not trip it.
        assert!(
            !rules("src/a.css", "@media (max-width: 600px){a{color:red}}")
                .contains(&"layout-transition")
        );
    }

    #[test]
    fn flags_hairline_plus_wide_shadow_on_the_same_rule() {
        assert!(rules(
            "src/a.css",
            ".card{border:1px solid #eee;box-shadow:0 4px 24px rgba(0,0,0,.1)}"
        )
        .contains(&"hairline-plus-wide-shadow"));
        // Border-led alone (a tight shadow) is a coherent elevation language.
        assert!(!rules(
            "src/a.css",
            ".card{border:1px solid #eee;box-shadow:0 1px 2px rgba(0,0,0,.05)}"
        )
        .contains(&"hairline-plus-wide-shadow"));
    }

    #[test]
    fn flags_dark_glow() {
        assert!(rules(
            "src/a.css",
            ".panel{background:#0a0a0b;box-shadow:0 0 40px rgba(99,220,180,0.4)}"
        )
        .contains(&"dark-glow"));
        // A neutral shadow on dark is fine.
        assert!(!rules(
            "src/a.css",
            ".panel{background:#0a0a0b;box-shadow:0 0 40px rgba(0,0,0,0.6)}"
        )
        .contains(&"dark-glow"));
    }

    #[test]
    fn flags_crushed_tracking_tiny_text_and_tight_leading() {
        assert!(rules("src/a.css", "h1{letter-spacing:-0.06em}").contains(&"crushed-tracking"));
        assert!(!rules("src/a.css", "h1{letter-spacing:-0.02em}").contains(&"crushed-tracking"));
        assert!(rules("src/a.css", ".x{font-size:10px}").contains(&"tiny-text"));
        assert!(!rules("src/a.css", ".x{font-size:14px}").contains(&"tiny-text"));
        assert!(rules("src/a.css", "p{font-size:16px;line-height:1.1}").contains(&"tight-leading"));
        // Display type legitimately goes below 1.3 — not flagged.
        assert!(
            !rules("src/a.css", "h1{font-size:64px;line-height:0.95}").contains(&"tight-leading")
        );
    }

    #[test]
    fn flags_zindex_magic_and_numbered_markers() {
        assert!(rules("src/a.css", ".modal{z-index:9999}").contains(&"zindex-magic"));
        assert!(!rules("src/a.css", ".modal{z-index:40}").contains(&"zindex-magic"));
        assert!(rules_for(
            "src/a.tsx",
            "<p>01 Discover</p><p>02 Design</p><p>03 Deliver</p>",
            Register::Brand
        )
        .contains(&"numbered-section-markers"));
    }

    #[test]
    fn flags_buzzwords_metrics_placeholders_and_em_dashes() {
        assert!(rules(
            "src/a.tsx",
            "<p>Supercharge your workflow with our industry-leading platform</p>"
        )
        .contains(&"marketing-buzzword"));
        assert!(!rules("src/a.tsx", "<p>supercharge</p>").contains(&"marketing-buzzword"));
        assert!(rules("src/a.tsx", "Trusted by 50,000+ teams").contains(&"invented-metrics"));
        assert!(rules("src/a.tsx", "<p>Jane Doe, CEO</p>").contains(&"placeholder-name"));
        assert!(rules("src/a.tsx", "a — b — c — d — e — f").contains(&"em-dash-overuse"));
        assert!(!rules("src/a.tsx", "a — b").contains(&"em-dash-overuse"));
    }

    #[test]
    fn flags_cream_and_pure_bw_surfaces() {
        assert!(rules("src/a.css", "body{background:#faf3e6}").contains(&"cream-band"));
        assert!(!rules("src/a.css", "body{background:#fafafa}").contains(&"cream-band"));
        assert!(rules("src/a.css", "body{background:#fff}").contains(&"pure-bw-surface"));
        assert!(rules("src/a.css", "body{background-color:#000000}").contains(&"pure-bw-surface"));
        // Pure white as TEXT on a dark fill is legit.
        assert!(
            !rules("src/a.css", ".btn{color:#fff;background:#0a0a0b}").contains(&"pure-bw-surface")
        );
        // A hex that merely starts with the pure digits is not a false match.
        assert!(!rules("src/a.css", "body{background:#0001ff}").contains(&"pure-bw-surface"));
    }

    // ── clean code + fail-open ────────────────────────────────────────────

    #[test]
    fn a_clean_product_ui_passes_in_the_product_register() {
        let css = ".card{background:var(--color-card);color:var(--color-on-card);\
                   border:1px solid var(--color-border);border-radius:8px;\
                   font-family:Inter,ui-sans-serif;font-size:14px;line-height:1.5;\
                   font-weight:500;transition:transform 120ms cubic-bezier(0.2,0,0.2,1)}";
        assert!(
            scan_design_rules("src/a.css", css, Register::Product).is_empty(),
            "clean product css: {:?}",
            rules_for("src/a.css", css, Register::Product)
        );
    }

    #[test]
    fn a_clean_brand_ui_passes_in_the_brand_register() {
        let css = "h1{font-family:\"Clash Display\",system-ui;color:var(--color-on-bg);\
                   font-size:64px;font-weight:600;line-height:1.05;\
                   transition:transform 200ms cubic-bezier(0.16,1,0.3,1)}\
                   p{font-size:18px;line-height:1.6}";
        assert!(
            scan_design_rules("src/a.css", css, Register::Brand).is_empty(),
            "clean brand css: {:?}",
            rules_for("src/a.css", css, Register::Brand)
        );
    }

    #[test]
    fn register_parses_tolerantly_and_defaults_to_unknown() {
        assert_eq!(Register::parse("brand"), Register::Brand);
        assert_eq!(Register::parse("Landing page"), Register::Brand);
        assert_eq!(Register::parse("product"), Register::Product);
        assert_eq!(Register::parse("admin dashboard"), Register::Product);
        assert_eq!(Register::parse("whatever"), Register::Unknown);
        assert_eq!(Register::parse(""), Register::Unknown);
        // A mixed declaration is UNKNOWN — every rule runs (fail-open).
        assert_eq!(Register::parse("brand + product"), Register::Unknown);
    }

    #[test]
    fn ignores_non_ui_files_and_empty_input() {
        assert!(scan_design_quality("README.md", "#6366f1 supercharge 10x faster").is_empty());
        assert!(scan_design_quality("src/a.tsx", "").is_empty());
    }

    #[test]
    fn never_panics_on_multibyte_input() {
        let cases = [
            "比对手快10x more 的体验".to_string(),
            format!("font-family: {}", "标".repeat(60)),
            format!("font-family:{} sans", "黑体".repeat(40)),
            "标标标9x more".to_string(),
            "图".repeat(500),
            format!("background:{}", "色".repeat(80)),
            format!("font-weight:{}", "粗".repeat(40)),
            format!("font-size:{}px大", "字".repeat(40)),
            "/* 主题 */ .x{color:#你好;background:#fffaf0}".to_string(),
            "#abcdeé".to_string(),
            "色 #你好世界 #fffffé end".to_string(),
        ];
        for c in &cases {
            for reg in [Register::Brand, Register::Product, Register::Unknown] {
                let _ = scan_design_rules("src/a.css", c, reg);
            }
            let _ = extract_fonts(c);
        }
    }

    #[test]
    fn extract_fonts_collects_declared_families() {
        let css = "h1{font-family:\"Clash Display\", Inter, sans-serif} \
                   body{--font-mono: 'Geist Mono', monospace}";
        let fonts = extract_fonts(css);
        assert!(fonts.contains(&"clash display".to_string()));
        assert!(fonts.contains(&"inter".to_string()));
        assert!(fonts.contains(&"geist mono".to_string()));
        assert!(is_generic_font("sans-serif") && is_generic_font("monospace"));
        assert!(!is_generic_font("clash display"));
    }
}
