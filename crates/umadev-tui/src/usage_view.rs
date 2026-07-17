//! Shared, quality-aware formatter for CLI and TUI usage reports.

use umadev_agent::runner::{format_usd_ticks, CostBreakdown, TokenBreakdown, UsageReport};
use umadev_i18n::{t, tf, Lang};

fn token_summary(lang: Lang, tokens: TokenBreakdown) -> String {
    let mut parts = Vec::new();
    if tokens.exact_calls > 0 {
        parts.push(tf(
            lang,
            "usage.tokens_exact",
            &[
                &tokens.exact_tokens.to_string(),
                &tokens.exact_calls.to_string(),
            ],
        ));
    }
    if tokens.lower_bound_calls > 0 {
        parts.push(tf(
            lang,
            "usage.tokens_lower_bound",
            &[
                &tokens.lower_bound_tokens.to_string(),
                &tokens.lower_bound_calls.to_string(),
            ],
        ));
    }
    if tokens.estimated_calls > 0 {
        parts.push(tf(
            lang,
            "usage.tokens_estimated",
            &[
                &tokens.estimated_tokens.to_string(),
                &tokens.estimated_calls.to_string(),
            ],
        ));
    }
    if tokens.unknown_calls > 0 {
        parts.push(tf(
            lang,
            "usage.tokens_unknown",
            &[&tokens.unknown_calls.to_string()],
        ));
    }
    if parts.is_empty() {
        t(lang, "usage.tokens_none").to_string()
    } else {
        parts.join(" · ")
    }
}

fn cost_summary(lang: Lang, cost: CostBreakdown, total_calls: u64) -> String {
    if let Some(total) = cost.complete_total_usd_ticks() {
        return tf(lang, "usage.cost_exact", &[&format_usd_ticks(total)]);
    }
    if cost.exact_calls > 0 {
        return tf(
            lang,
            "usage.cost_partial",
            &[
                &format_usd_ticks(cost.reported_usd_ticks),
                &cost.exact_calls.to_string(),
                &total_calls.to_string(),
            ],
        );
    }
    t(lang, "usage.cost_unknown").to_string()
}

/// Render one usage report without inventing token precision or provider cost.
///
/// Exact, lower-bound, estimated, and unknown token buckets remain separate at
/// every level. Cost is shown only when the selected base reported it.
#[must_use]
pub fn format_usage_report(lang: Lang, report: &UsageReport) -> String {
    if report.is_empty() {
        return t(lang, "usage.empty").to_string();
    }
    let mut out = tf(
        lang,
        "usage.title",
        &[
            &report.total_calls.to_string(),
            &report.runs.len().to_string(),
        ],
    );
    out.push('\n');
    for run in &report.runs {
        let backends = if run.backends.is_empty() {
            "offline".to_string()
        } else {
            run.backends.join(", ")
        };
        out.push('\n');
        out.push_str(&tf(
            lang,
            "usage.run_header",
            &[&run.index.to_string(), &backends],
        ));
        out.push('\n');
        for phase in &run.phases {
            out.push_str(&tf(
                lang,
                "usage.phase_line",
                &[
                    &phase.phase,
                    &phase.calls.to_string(),
                    &token_summary(lang, phase.token_breakdown),
                ],
            ));
            out.push('\n');
        }
        out.push_str(&tf(
            lang,
            "usage.run_total",
            &[
                &run.calls.to_string(),
                &token_summary(lang, run.token_breakdown),
            ],
        ));
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&tf(
        lang,
        "usage.grand_total",
        &[&token_summary(lang, report.token_breakdown)],
    ));
    out.push('\n');
    out.push_str(&cost_summary(
        lang,
        report.cost_breakdown,
        report.total_calls,
    ));
    if report.migrated_v1_calls > 0 {
        out.push('\n');
        out.push_str(&tf(
            lang,
            "usage.migrated_v1",
            &[&report.migrated_v1_calls.to_string()],
        ));
    }
    if report.corrupt_rows > 0 {
        out.push('\n');
        out.push_str(&tf(
            lang,
            "usage.corrupt_rows",
            &[&report.corrupt_rows.to_string()],
        ));
    }
    out.push('\n');
    out.push_str(t(lang, "usage.note_quality"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use umadev_agent::runner::{PhaseUsage, RunUsage};

    fn report(tokens: TokenBreakdown, cost: CostBreakdown) -> UsageReport {
        let phase = PhaseUsage {
            phase: "implementation".to_string(),
            calls: 2,
            tokens: u64::try_from(tokens.known_numeric_sum()).unwrap_or(u64::MAX),
            token_breakdown: tokens,
            cost_breakdown: cost,
        };
        let run = RunUsage {
            index: 1,
            backends: vec!["grok-build".to_string()],
            phases: vec![phase],
            calls: 2,
            tokens: u64::try_from(tokens.known_numeric_sum()).unwrap_or(u64::MAX),
            token_breakdown: tokens,
            cost_breakdown: cost,
        };
        UsageReport {
            runs: vec![run],
            total_calls: 2,
            total_tokens: u64::try_from(tokens.known_numeric_sum()).unwrap_or(u64::MAX),
            token_breakdown: tokens,
            cost_breakdown: cost,
            backends: vec!["grok-build".to_string()],
            corrupt_rows: 0,
            migrated_v1_calls: 0,
        }
    }

    #[test]
    fn exact_report_never_uses_fabricated_flat_rate() {
        let output = format_usage_report(
            Lang::En,
            &report(
                TokenBreakdown {
                    exact_tokens: 150,
                    exact_calls: 2,
                    ..TokenBreakdown::default()
                },
                CostBreakdown {
                    reported_usd_ticks: 2_500_000_000,
                    exact_calls: 2,
                    unknown_calls: 0,
                },
            ),
        );
        assert!(output.contains("exact 150"));
        assert!(output.contains("Base-reported exact cost: $0.25"));
        assert!(!output.contains("NaN"));
        assert!(!output.contains("$3/M"));
        assert!(!output.contains("Rough cost estimate"));
    }

    #[test]
    fn mixed_quality_buckets_and_partial_cost_stay_separate() {
        let output = format_usage_report(
            Lang::ZhCn,
            &report(
                TokenBreakdown {
                    exact_tokens: 100,
                    exact_calls: 1,
                    lower_bound_tokens: 40,
                    lower_bound_calls: 1,
                    estimated_tokens: 30,
                    estimated_calls: 1,
                    unknown_calls: 1,
                },
                CostBreakdown {
                    reported_usd_ticks: 100_000_000,
                    exact_calls: 1,
                    unknown_calls: 1,
                },
            ),
        );
        assert!(output.contains("精确 100"));
        assert!(output.contains("至少 40"));
        assert!(output.contains("约 30"));
        assert!(output.contains("1 次未知"));
        assert!(output.contains("覆盖 1/2 次调用"));
    }

    #[test]
    fn missing_cost_is_explicitly_unknown() {
        let output = format_usage_report(
            Lang::En,
            &report(
                TokenBreakdown {
                    unknown_calls: 2,
                    ..TokenBreakdown::default()
                },
                CostBreakdown {
                    unknown_calls: 2,
                    ..CostBreakdown::default()
                },
            ),
        );
        assert!(output.contains("Cost: unknown"));
        assert!(!output.contains('$'));
    }
}
