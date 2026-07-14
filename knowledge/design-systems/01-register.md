---
id: 01-register
title: Design Register — Brand vs Product
domain: design-systems
category: design-systems
difficulty: intermediate
tags: [register, brand, product, dashboard, admin, devtool, landing, marketing, design-systems, governance]
quality_score: 78
last_updated: 2026-07-14
---
# Design Register — Brand vs Product

> **Use this FIRST, before any other design-systems file.** Every visual rule in
> this corpus is scoped to a REGISTER. Applying the wrong register's rules is the
> most expensive design mistake we make: it dresses a dashboard like a landing page
> and makes the product measurably worse to use.

## Trigger

Read this whenever you are about to write UI, pick a design pack, author
`design-tokens.{json,css}`, or review a UI diff. Declare the register in the UIUX
doc's `## Visual direction` section **before** you choose a single color.

## The two registers

**brand** — landing page, marketing site, campaign, launch page, portfolio,
pitch/press page, brand showcase.
Here **design IS the product**. The page has one job: be memorable in six seconds.
Distinctiveness beats familiarity. A visitor who cannot tell which product the
screenshot belongs to means you shipped a template.

**product** — app, dashboard, admin, console, settings, editor, devtool,
data table, internal tool, any surface a user returns to daily.
Here **design SERVES the task**. The user did not come to admire the interface;
they came to finish something. Familiarity beats novelty, because every gram of
novelty is a gram of relearning. Density is a virtue. The best compliment is that
nobody mentioned the UI.

**Ambiguous?** Ask: *does the user arrive to DECIDE, or to DO?* Decide → brand.
Do → product. A marketing site with an embedded demo app is BOTH: the shell is
brand register, the demo pane is product register. Register is a per-surface
property, not a per-repo one.

## What flips between registers

| Dimension | brand | product |
|---|---|---|
| Typeface | a distinctive display face is required; a system-font-only page is a defect | a familiar neutral UI/system face is CORRECT and preferred; a display face is a defect on a data table |
| Type scale | dramatic jumps (2.5x–4x display:body) carry the hierarchy | fixed rem scale, step ratio **1.125–1.2**; hierarchy comes from weight, color, and spacing |
| Weights | extremes are a tool (200 vs 800) | 400 / 500 / 600 only; extremes read as shouting |
| Color | one dominant + one sharp accent; committed | restrained is the FLOOR; color is a semantic signal (status, selection, danger), not decoration |
| Background | depth is allowed (grain, geometry, layered fields) | flat surface tokens; a decorative background steals contrast from data |
| Motion | ONE orchestrated page-load reveal is a signature | **no page-load choreography.** Motion only confirms a user action (≤150ms) or covers a state change |
| Density | generous negative space OR controlled density — pick a binary | density is a FEATURE: minimize travel, fit more true rows per screen |
| Novel layout | one unconventional section is required | conventional placement wins; put the nav where the user's hand already is |
| Imagery | photography / illustration carries meaning | icons + data; decorative imagery is noise |

## Rules that hold in BOTH registers

These are register-independent — never relax them:

1. Colors, fonts, spacing, radii, and motion durations all come from tokens. No
   raw hex, no one-off inline style.
2. Every surface token ships a paired `on-` foreground, and the pair meets WCAG
   (4.5:1 body, 3:1 large/UI).
3. **One** icon library, **one** stroke weight. Never mix libraries; never emoji as
   an icon; never a hand-rolled decorative SVG. *Which* library is a per-pack choice.
4. Real representative content — never lorem, never invented metrics.
5. Every interactive element has hover / focus-visible / active / disabled states,
   and every async surface has loading + empty + error states.
6. No AI-purple (`oklch` hue 270–320 at chroma ≥ 0.09) as primary or accent unless
   the requirement explicitly asks for purple.

## Anti-patterns — with the observable tell

| Anti-pattern | Observable tell | Positive target |
|---|---|---|
| Marketing type in a product | ≥3 font sizes whose max/min ratio ≥ 2.0 inside an app route | Fixed scale, ratio 1.125–1.2; carry hierarchy with weight + color |
| Landing-page motion in an app | `animate-*` / staggered `delay-*` on a dashboard route's mount | Motion only on user action, ≤150ms |
| Decorative background under data | gradient / mesh / grain declared on a route that renders a table or chart | Flat `--color-bg`; spend contrast on the data |
| Over-rounded product chrome | `border-radius` ≥ 24px on a card / input / section | 6–12px; keep the eye on content, not on the container |
| Airy space where density was asked for | ≥64px vertical rhythm between rows in a list/table view | 4pt scale, 8–16px row rhythm; more true rows per screen |
| Grey-on-grey "clean" product UI | body text under 4.5:1 against its surface | Use the paired `on-` token; measure, don't eyeball |
| System-font landing page | landing route's display face resolves to `system-ui` / `-apple-system` only | Commit to one distinctive display face + one body face |
| Flat landing hierarchy | landing hero heading under 2.5x the body size | Let the display step be dramatic; that's what the register is for |

## Completion criteria (check every one before you call the UI done)

- [ ] The UIUX doc's `## Visual direction` names the register in one word: `brand` or `product`.
- [ ] The chosen design pack's `register:` field includes that register.
- [ ] Every rule you applied from `00-craft-rules.md` / `anti-ai-slop.md` marked
      *brand-only* was skipped if the register is `product`.
- [ ] Type scale: brand → the display:body ratio is ≥ 2.5; product → every adjacent
      step ratio is within 1.125–1.2.
- [ ] Motion: brand → at most ONE orchestrated entrance; product → zero mount
      animations; every transition is ≤ 150ms and tied to an interaction.
- [ ] Token file declares ≥ 6 color roles, each with a paired `on-` foreground.
- [ ] Contrast measured, not eyeballed: 4.5:1 body, 3:1 large/UI, for every pair.
- [ ] One icon library, one stroke weight, named in the UIUX doc.
