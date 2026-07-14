//! **Who authorized this color family?** — the one question, asked of the brain.
//!
//! The anti-AI-slop floor default-REJECTS the indigo/violet band (`ai-color-palette`,
//! the token-level banned-hue rule, and the pre-write governor all read it). It is a
//! default, not a censor: a user who says *"our brand color is violet `#7c3aed`"* has
//! ANSWERED the question the rule exists to ask, and a build that blocks them cannot
//! converge — the design floor accepts the tokens the write governor refuses.
//!
//! So something has to decide *"did the requirement authorize this hue?"*. That used to
//! be a lexical reader: scan the sentence for negation words, intent words, proper nouns,
//! code fences. It could not converge. Six review rounds, six leaks — each one shipping
//! the canonical AI hero gradient into a repo whose brief said, in so many words, *stop*:
//!
//! - "We are **banning** purple from the theme entirely." (`ban` was in the list;
//!   `banning` was not)
//! - 「紫色被客户**否决**了，用蓝绿色」 (`否` was not in the CJK list — while the English
//!   twin *"purple was vetoed by the client"* WAS caught: the zh half of a zh-first product
//!   leaking where the en half held)
//! - 「紫色主题要**删掉**，用蓝色」/「紫色渐变全部**移除**」/「旧站的紫色主色**去掉**」
//!   (the forward scan consulted one CJK list and not its twin)
//!
//! Every fix was another word, and the next phrasing was always outside it. **A prohibition
//! has unboundedly many phrasings; a word list is the wrong shape of answer.**
//!
//! ## The project's standing rule: intent is judged by the brain
//!
//! UmaDev does not classify intent with keywords. [`crate::router::route_via_brain`] does
//! not guess chat-vs-edit-vs-build from a word table — it asks the base's own model, once,
//! and the model is authoritative. *"Did the user authorize this color family?"* is the
//! same class of question, and it gets the same answer: **one stateless, structured consult
//! → a typed [`ColorPermission`]**. No keyword classifier, and nothing that grows.
//!
//! ## Fail direction: STRICT (and why that is not a fail-open violation)
//!
//! Brain unreachable, offline runtime, fork refused, prose instead of JSON, a timeout, a
//! malformed answer — every one of them returns [`ColorPermission::withheld`] and the rule
//! stays ARMED. This is the OPPOSITE of the router's degradation (which falls to the
//! lightest path), and deliberately so: the two failures are not symmetric. A leak writes
//! AI-slop into the customer's repository, irreversibly, and nobody sees it until review; a
//! false block is one loud, recoverable rework. So the tie goes to armed.
//!
//! The governance contract is untouched. Governance is fail-open in the sense that
//! *a governor bug must never block or crash the HOST* — and nothing here can: a withheld
//! permission never errors, never panics, never stops the base from running. It only
//! declines to stand a rule DOWN. Refusing to disarm a safety check is not the same act as
//! blocking the host, and only the second one is forbidden.
//!
//! ## Computed ONCE, persisted, never re-derived
//!
//! The consult runs at the run door, the instant the requirement is known and before the
//! first file is written ([`crate::director_loop`]'s `persist_run_governance_context` →
//! [`crate::planner::persist_project_context_with_color`]), and the verdict is written into
//! `.umadev/governance-context.json`. Three surfaces then READ that stored decision — the
//! out-of-process PreToolUse hook (spawned per base tool call), `umadev ci` (spawned from
//! `.git/hooks/pre-commit`), and the design floor. **None of them may consult the brain**:
//! they have none, they run in foreign processes, and a per-write model call would be
//! absurd. One decision, one writer, many readers.

use umadev_runtime::BaseSession;

/// The brain's verdict on the flagged color family — a typed answer to one question.
///
/// Generalizes cleanly: today the floor flags exactly one family (the AI indigo/violet
/// band), so this carries one permission plus the brain's own one-line justification. A
/// second flagged family becomes a second field and a second line in the prompt — never a
/// second word list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorPermission {
    /// The requirement affirmatively authorized a purple / violet / indigo brand — so the
    /// `ai-color-palette` default-reject stands down for this run.
    ///
    /// `false` means *permission withheld*, which is both "the user forbade it" and "we
    /// could not establish that they allowed it". The floor treats those identically, and
    /// must: only an affirmative authorization may disarm a safety default.
    pub purple_allowed: bool,
    /// Why — the brain's own short justification, or the reason permission was withheld
    /// without asking. Diagnostic only; it drives no logic. Surfaced in the audit trail so
    /// a user who is blocked on a color can see what the run understood them to have said.
    pub reason: String,
}

impl ColorPermission {
    /// Permission WITHHELD — the rule stays armed. Every failure path lands here.
    #[must_use]
    pub fn withheld(reason: impl Into<String>) -> Self {
        Self {
            purple_allowed: false,
            reason: reason.into(),
        }
    }

    /// Permission GRANTED — the user authorized the hue, so the default-reject stands down.
    #[must_use]
    pub fn granted(reason: impl Into<String>) -> Self {
        Self {
            purple_allowed: true,
            reason: reason.into(),
        }
    }
}

impl Default for ColorPermission {
    /// The strict posture: withheld, un-consulted.
    fn default() -> Self {
        Self::withheld("not consulted")
    }
}

/// The one structured question. Terse, decisive, and explicitly biased toward WITHHOLDING —
/// the model is told that silence, ambiguity, a mere mention, and a prohibition all mean the
/// same thing, so the only path to `true` is an affirmative authorization.
///
/// It names the shapes that a lexical reader kept getting wrong, in BOTH scripts, not as a
/// word list to match but as the *kinds of sentence* to reason about: a verdict passed on
/// the hue, a removal, a replacement, a company name that happens to be a color, a hex
/// quoted inside a code fence as the example of what to avoid. A model reads all of these
/// correctly; a `contains()` never could.
const COLOR_PERMISSION_SYSTEM: &str = "You decide ONE question about a software requirement, \
     and nothing else. \
     Question: does the requirement AFFIRMATIVELY AUTHORIZE a purple / violet / indigo brand \
     color (the hue family that includes #7c3aed, #667eea, #764ba2, rgb(124,58,237), 紫, 靛)? \
     Answer `true` ONLY when the user is CHOOSING that hue for the product — \"our brand is \
     violet\", \"primary: #7c3aed\", \"make the hero purple\", \"主色调用紫色\", \"品牌主色用紫色\". \
     Answer `false` for EVERYTHING else. In particular `false` when: \
     (a) the requirement FORBIDS the hue, in any phrasing whatsoever — \"do not use purple\", \
     \"avoid AI-looking templates (purple gradient, ...)\", \"we are banning purple\", \
     \"purple is off-limits / prohibited / disallowed / ruled out\", \"不要用紫色\", \"禁止紫色\"; \
     (b) someone passed a VERDICT on it — \"the client rejected purple\", \"purple was vetoed\", \
     \"紫色被客户否决了\", \"紫色方案驳回\"; \
     (c) the hue is named in order to be REMOVED or REPLACED — \"replace the old #7c3aed with \
     our new blue\", \"drop the purple\", \"retire the violet theme\", \"紫色主题要删掉\", \
     \"紫色渐变全部移除\", \"旧站的紫色主色去掉,换成蓝色\", \"弃用紫色\"; \
     (d) the word is part of a PROPER NOUN, not a color choice — \"IndiGo Airlines\", \
     \"Violet Capital\"; \
     (e) the hue appears only inside QUOTED CODE / an example of what NOT to do; \
     (f) the requirement simply does not settle the question. \
     A requirement may remove one thing and CHOOSE the hue for another — \"去掉表情图标,把主色改成\
     紫色\" / \"make the hero purple and remove the emoji icons\" — that is `true`: what matters is \
     whether the hue itself is the thing being chosen or the thing being taken away. \
     Word order carries no meaning here; the two scripts are judged identically. \
     When you are not sure, answer `false`. \
     Reply with EXACTLY ONE JSON object and nothing else: \
     {\"purple_allowed\":true|false,\"reason\":\"<one short clause>\"}";

/// Ask the brain whether `requirement` authorizes the flagged color family.
///
/// One read-only forked consult, one strict-JSON answer, one typed verdict. The main
/// (single-writer) session is never touched, and nothing on disk changes — the caller
/// persists the result.
///
/// **Cheap by construction.** The lexical pre-filter
/// ([`umadev_governance::requirement_mentions_flagged_color`]) short-circuits the
/// overwhelmingly common case — a requirement that never names the hue at all cannot have
/// authorized it, so there is nothing to ask and no consult is spent. That pre-filter can
/// only ARM: a `false` from it means "withheld, don't ask", never "granted". It is not a
/// classifier and it does not grow.
///
/// **Fail-strict at every edge** (see the module docs): a fork that will not open, an
/// offline brain, a timeout, prose instead of JSON, a malformed object — all of them return
/// [`ColorPermission::withheld`]. Never errors, never panics, never blocks the host.
pub async fn consult_color_permission(
    session: &mut dyn BaseSession,
    requirement: &str,
) -> ColorPermission {
    // The only thing the lexer is still allowed to decide, and it decides it in the ARMED
    // direction: nobody can authorize a hue they never named.
    if !umadev_governance::requirement_mentions_flagged_color(requirement) {
        return ColorPermission::withheld("the requirement never names the flagged hue");
    }

    let fork = crate::continuous::fork_with_timeout(session).await;
    let consult = crate::continuous::ForkConsult::new(fork);
    let answer = consult
        .judge_json(
            "color-permission",
            COLOR_PERMISSION_SYSTEM,
            format!("Requirement:\n{requirement}"),
        )
        .await;
    consult.end().await;

    let Some(json) = answer else {
        // No usable answer from the brain. STRICT: a permission we could not establish is a
        // permission not granted. (The router degrades the other way, to the lightest path —
        // because there the cost of being wrong is a cheap turn, not slop in the repo.)
        return ColorPermission::withheld("the brain gave no usable verdict — permission withheld");
    };
    parse_color_permission(&json).unwrap_or_else(|| {
        ColorPermission::withheld("the brain's verdict could not be parsed — permission withheld")
    })
}

/// Parse the brain's strict-JSON verdict. `None` when the object is missing the decision or
/// carries it as the wrong type — which the caller turns into a WITHHELD permission, never a
/// granted one.
///
/// Deliberately narrow: only a real JSON `true` on `purple_allowed` grants. A string
/// `"true"`, a `1`, a missing field, or a nested shape does not — an ambiguous answer to a
/// permission question is a refusal.
fn parse_color_permission(json: &str) -> Option<ColorPermission> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let allowed = v.get("purple_allowed")?.as_bool()?;
    let reason = v
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect::<String>();
    Some(ColorPermission {
        purple_allowed: allowed,
        reason: if reason.is_empty() {
            "the brain gave no reason".to_string()
        } else {
            reason
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use umadev_runtime::{ApprovalDecision, SessionError, SessionEvent, TurnStatus};

    /// Every sentence the old lexical reader was ever asked to judge, plus the five it was
    /// PROVEN to leak on. `true` = the requirement authorizes the hue; `false` = it does not.
    ///
    /// This is the battery that used to test a word list. It now tests the PATH: that each of
    /// these reaches the brain (the pre-filter never silently short-circuits a judgeable
    /// sentence), that the brain's verdict is what lands, and — the load-bearing one — that
    /// with no brain, not one of them grants.
    const BATTERY: &[(&str, bool)] = &[
        // ── PERMIT: the user is choosing the hue ──────────────────────────────────────
        ("our brand is violet", true),
        ("#7c3aed is our primary", true),
        ("primary: rgb(124, 58, 237), no exceptions", true),
        ("brand = oklch(0.55 0.22 290)", true),
        ("make the whole thing purple", true),
        ("make the hero purple with a deep indigo accent", true),
        ("brand palette: violet + slate", true),
        ("Use a purple gradient", true),
        ("use purple instead of blue", true),
        ("make the hero purple and remove the emoji icons", true),
        ("primary brand color #7c3aed, and drop the old logo", true),
        ("No emoji icons anywhere. Our brand color is violet.", true),
        (
            "Our brand color is violet. Do not use emoji icons anywhere.",
            true,
        ),
        ("our brand color is #7c3aed", true),
        ("remove the emoji icons, make the hero purple", true),
        (
            "replace the old logo, and use violet as the primary color",
            true,
        ),
        // zh — the same choices, in the script this product leads with.
        ("主色调用紫色", true),
        ("使用紫色作为主色", true),
        ("把主色设为 #7c3aed", true),
        ("把主色改成紫色,并去掉表情图标", true),
        // A removal aimed at a DIFFERENT noun, with the hue chosen after it. zh puts the
        // removal clause FIRST by default, so this is the ORDINARY affirmative phrasing —
        // and a symmetric lexical scan blocked it. The brain reads it correctly.
        ("去掉表情图标，把主色改成紫色", true),
        ("删除旧的图标，品牌主色用紫色", true),
        ("去掉表情图标,品牌主色改成 #7c3aed", true),
        // ── REJECT: a prohibition, in every shape it actually gets written ────────────
        (
            "Do NOT use purple. No purple gradients, avoid the AI look.",
            false,
        ),
        (
            "avoid AI-looking templates (purple gradient, emoji icons, default-font-only)",
            false,
        ),
        ("no violet, no indigo, no lavender anywhere", false),
        ("purple is not our brand", false),
        ("without any purple or pink gradients", false),
        ("the background must never be #7c3aed", false),
        (
            "Please avoid, throughout the entire application and all of its many screens, \
             the color purple",
            false,
        ),
        ("不要用紫色渐变,别做成 AI 味", false),
        ("背景绝不能是紫色", false),
        // A VERDICT passed on the hue.
        ("the client rejected purple, use teal", false),
        ("purple was vetoed by the client, use teal", false),
        // A REMOVAL / REPLACEMENT — the hue is named precisely in order to be rid of it.
        (
            "replace the old #7c3aed with our new #0ea5e9 brand blue",
            false,
        ),
        ("remove every #7c3aed from the legacy stylesheet", false),
        ("remove the purple gradient, and make it teal", false),
        ("replace the violet accent, use sky blue", false),
        ("我们要去掉旧站的 #7c3aed", false),
        ("去掉紫色，改用蓝色", false),
        ("删除紫色主题,换成青色", false),
        // A PROPER NOUN is not a color decision.
        ("a booking app for IndiGo Airlines", false),
        ("a fintech app for Violet Capital", false),
        ("the Indigo Partners investor portal", false),
        // QUOTED CODE — the user's own counter-example, handed back to them as consent.
        ("do not use purple.\n```css\n:root{--x:#7c3aed}\n```", false),
        ("avoid the AI look:\n```css\n--brand: #7c3aed;\n```", false),
        // ── The FIVE PROVEN LEAKS. Each one shipped the #667eea→#764ba2 AI hero gradient
        //    into a repo whose requirement said the opposite. Each was a word absent from a
        //    list (`banning`, `否`) or a list consulted in only one direction.
        ("We are banning purple from the theme entirely.", false),
        ("紫色被客户否决了，用蓝绿色", false),
        ("紫色主题要删掉，用蓝色", false),
        ("紫色渐变全部移除，主色改为中性灰", false),
        ("旧站的紫色主色去掉，换成蓝色", false),
    ];

    // ── A stub brain. The main session is never driven — only forked. ────────────────

    struct StubFork {
        reply: String,
        pending: VecDeque<SessionEvent>,
    }
    #[async_trait::async_trait]
    impl BaseSession for StubFork {
        async fn send_turn(&mut self, _d: String) -> Result<(), SessionError> {
            self.pending = [
                SessionEvent::TextDelta(self.reply.clone()),
                SessionEvent::TurnDone {
                    status: TurnStatus::Completed,
                    usage: None,
                },
            ]
            .into_iter()
            .collect();
            Ok(())
        }
        async fn next_event(&mut self) -> Option<SessionEvent> {
            self.pending.pop_front()
        }
        async fn respond(&mut self, _r: &str, _d: ApprovalDecision) -> Result<(), SessionError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
        async fn end(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
    }

    /// A base whose read-only fork answers the color question with `reply`. `can_fork: false`
    /// is the unreachable-brain path (the STRICT floor).
    struct StubBrain {
        reply: String,
        can_fork: bool,
    }
    impl StubBrain {
        fn answering(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                can_fork: true,
            }
        }
        /// The brain is gone: no fork, no verdict, no permission.
        fn unreachable() -> Self {
            Self {
                reply: String::new(),
                can_fork: false,
            }
        }
        /// The verdict a competent model returns for `allowed`.
        fn verdict(allowed: bool) -> Self {
            Self::answering(&format!(
                "{{\"purple_allowed\":{allowed},\"reason\":\"judged from the requirement\"}}"
            ))
        }
    }
    #[async_trait::async_trait]
    impl BaseSession for StubBrain {
        async fn fork(&mut self) -> Result<Box<dyn BaseSession>, SessionError> {
            if !self.can_fork {
                return Err(SessionError::ForkUnsupported("test".into()));
            }
            Ok(Box::new(StubFork {
                reply: self.reply.clone(),
                pending: VecDeque::new(),
            }))
        }
        async fn send_turn(&mut self, _d: String) -> Result<(), SessionError> {
            Ok(())
        }
        async fn next_event(&mut self) -> Option<SessionEvent> {
            None
        }
        async fn respond(&mut self, _r: &str, _d: ApprovalDecision) -> Result<(), SessionError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
        async fn end(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_brains_verdict_is_what_lands_for_every_case_in_the_battery() {
        // The whole battery — 23 permits + 28 rejects, both scripts, including the five
        // sentences the word list PROVED to leak on — driven through the real path with a
        // brain that answers correctly. What this locks is that the path is FAITHFUL: the
        // verdict that comes back is the verdict that lands, for every shape of sentence,
        // with nothing lexical intercepting it on the way.
        for (requirement, allowed) in BATTERY {
            let mut brain = StubBrain::verdict(*allowed);
            let got = consult_color_permission(&mut brain, requirement).await;
            assert_eq!(
                got.purple_allowed, *allowed,
                "the brain's verdict must be the one that lands: {requirement:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_unreachable_brain_grants_nothing_in_the_entire_battery() {
        // THE FLOOR, and the test that would have caught all six leaks at once. With no brain
        // there is no permission — for any sentence, in any script, however affirmative. The
        // rule stays armed and the AI palette cannot ship. A false block here is one
        // recoverable rework; a leak is slop in the customer's repo, forever.
        for (requirement, _) in BATTERY {
            let mut brain = StubBrain::unreachable();
            let got = consult_color_permission(&mut brain, requirement).await;
            assert!(
                !got.purple_allowed,
                "no brain ⇒ no permission, ever: {requirement:?}"
            );
        }
    }

    #[tokio::test]
    async fn every_judgeable_sentence_reaches_the_brain() {
        // The pre-filter's ONE job is to skip the consult when the hue is not named at all.
        // It must never short-circuit a sentence the brain has to judge — least of all the
        // five leaks, whose whole failure mode was a lexer deciding what it had no business
        // deciding. Every case in the battery names the band, so every case defers.
        for (requirement, _) in BATTERY {
            assert!(
                umadev_governance::requirement_mentions_flagged_color(requirement),
                "the pre-filter must DEFER this to the brain, not decide it: {requirement:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_requirement_that_never_names_the_hue_costs_no_consult() {
        // The cheap path: nothing named the band, so nobody could have authorized it. The
        // permission is withheld WITHOUT asking — and the stub brain proves it was not asked
        // (it is configured to GRANT, and the answer is still `false`).
        for silent in [
            "a clean dashboard for our sales team",
            "primary is #0ea5e9 (sky blue)",
            "brand red #dc2626, nothing else",
            "做一个后台管理系统",
            "",
        ] {
            let mut generous = StubBrain::verdict(true);
            let got = consult_color_permission(&mut generous, silent).await;
            assert!(
                !got.purple_allowed,
                "no mention ⇒ no consult, no permission: {silent:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_malformed_verdict_withholds_rather_than_grants() {
        // Every way the brain can fail to answer the question is the SAME answer: withheld.
        // An ambiguous reply to a permission question is a refusal, never a grant.
        for reply in [
            "",                                          // silence
            "Sure! The user seems to want purple here.", // prose, no JSON
            "{}",                                        // JSON, no decision
            "{\"purple_allowed\":\"true\"}",             // the decision as a STRING
            "{\"purple_allowed\":1}",                    // …as a number
            "{\"allowed\":true}",                        // the wrong key
            "{\"purple_allowed\":true",                  // truncated
        ] {
            let mut brain = StubBrain::answering(reply);
            let got = consult_color_permission(&mut brain, "our brand is violet").await;
            assert!(
                !got.purple_allowed,
                "an unusable verdict withholds the permission: {reply:?}"
            );
        }
        // …and a well-formed grant, on the same requirement, still grants — the strictness
        // above is about UNUSABLE answers, not about refusing the user their own brand.
        let mut brain = StubBrain::verdict(true);
        assert!(
            consult_color_permission(&mut brain, "our brand is violet")
                .await
                .purple_allowed,
            "a clean affirmative verdict must still stand the rule down"
        );
    }

    #[test]
    fn the_prompt_asks_the_question_the_word_list_could_not_answer() {
        // The prompt is the contract. It must name the sentence SHAPES a lexer kept getting
        // wrong — a verdict, a removal, a replacement, a proper noun, quoted code — in BOTH
        // scripts, and it must state the default-reject direction explicitly.
        let p = COLOR_PERMISSION_SYSTEM;
        assert!(
            p.contains("AFFIRMATIVELY AUTHORIZE"),
            "the question is authorization, not mention"
        );
        assert!(
            p.contains("When you are not sure, answer `false`"),
            "the prompt states the default-reject direction"
        );
        for shape in [
            "FORBIDS",
            "VERDICT",
            "REMOVED",
            "REPLACED",
            "PROPER NOUN",
            "QUOTED CODE",
        ] {
            assert!(p.contains(shape), "the prompt names the `{shape}` shape");
        }
        // Both scripts, judged identically — the zh/en split is what made the last leak the
        // worst kind for a zh-first product.
        assert!(
            p.contains("紫") && p.contains("否决"),
            "the prompt is bilingual"
        );
        assert!(
            p.contains("the two scripts are judged identically"),
            "the prompt forbids a per-script rule"
        );
        assert!(
            p.contains("purple_allowed"),
            "the prompt fixes the JSON shape the parser reads"
        );
    }

    #[test]
    fn a_withheld_permission_is_the_default() {
        assert!(!ColorPermission::default().purple_allowed);
        assert!(!ColorPermission::withheld("x").purple_allowed);
        assert!(ColorPermission::granted("x").purple_allowed);
    }
}
