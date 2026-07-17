use super::memory_view::{
    curated_lesson_source, curated_lesson_status_key, pitfall_status_icon, pitfall_status_key,
};
use super::{
    pitfall_first_observed, push_lesson_field, push_pitfall_observations,
    push_wrapped_pitfall_line, wrap_lesson_message,
};

/// Render the concrete `/pitfalls` incident ledger. Curated rules are ignored;
/// they belong exclusively to `/lessons`.
pub(super) fn format_pitfalls_report(
    lang: umadev_i18n::Lang,
    report: &umadev_agent::LessonsReport,
) -> String {
    let efficacy = &report.efficacy;
    if report.incidents.is_empty() && report.unclassified_candidates.is_empty() {
        let mut messages = vec![umadev_i18n::t(lang, "pitfalls.empty").to_string()];
        if efficacy.quarantined_records > 0 {
            messages.push(umadev_i18n::tf(
                lang,
                "pitfalls.quarantine_note",
                &[
                    &efficacy.quarantined_records.to_string(),
                    &efficacy.quarantined_hits.to_string(),
                ],
            ));
        }
        return wrap_lesson_message(&messages.join("\n\n"));
    }

    let mut out = String::new();
    push_wrapped_pitfall_line(
        &mut out,
        &umadev_i18n::tf(
            lang,
            "pitfalls.summary_title",
            &[&efficacy.total.to_string()],
        ),
    );
    push_wrapped_pitfall_line(
        &mut out,
        &umadev_i18n::tf(
            lang,
            "pitfalls.summary_status",
            &[
                &efficacy.hypothesis.to_string(),
                &efficacy.corroborated.to_string(),
                &efficacy.validated.to_string(),
                &efficacy.invalidated.to_string(),
            ],
        ),
    );
    push_wrapped_pitfall_line(
        &mut out,
        &umadev_i18n::tf(
            lang,
            "pitfalls.summary_storage",
            &[
                &efficacy.unclassified_candidates.to_string(),
                &efficacy.unclassified_candidate_hits.to_string(),
                &efficacy.quarantined_records.to_string(),
                &efficacy.quarantined_hits.to_string(),
            ],
        ),
    );
    if efficacy.quarantined_records > 0 {
        out.push('\n');
        push_wrapped_pitfall_line(
            &mut out,
            &umadev_i18n::tf(
                lang,
                "pitfalls.quarantine_note",
                &[
                    &efficacy.quarantined_records.to_string(),
                    &efficacy.quarantined_hits.to_string(),
                ],
            ),
        );
    }

    let unknown = umadev_i18n::t(lang, "pitfalls.time.unknown");
    let legacy_missing = umadev_i18n::t(lang, "pitfalls.time.legacy_missing");
    let unverified = umadev_i18n::t(lang, "pitfalls.time.unverified");
    let not_recurred = umadev_i18n::t(lang, "pitfalls.time.not_recurred");
    for incident in &report.incidents {
        out.push('\n');
        let status = umadev_i18n::t(lang, pitfall_status_key(incident.status));
        let title = if incident.title.trim().is_empty() {
            unknown
        } else {
            incident.title.trim()
        };
        push_wrapped_pitfall_line(
            &mut out,
            &umadev_i18n::tf(
                lang,
                "pitfalls.item_header",
                &[
                    pitfall_status_icon(incident.status),
                    title,
                    &incident.hits.to_string(),
                    &incident.recent_evidence_count.to_string(),
                    status,
                ],
            ),
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.signature_prefix"),
            if incident.signature.trim().is_empty() {
                unknown
            } else {
                incident.signature.trim()
            },
        );
        if !incident.context.is_empty() {
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "pitfalls.context_prefix"),
                &incident.context.join(", "),
            );
        }
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.root_cause_prefix"),
            if incident.root_cause.trim().is_empty() {
                unknown
            } else {
                incident.root_cause.trim()
            },
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.fix_prefix"),
            if incident.fix.trim().is_empty() {
                unknown
            } else {
                incident.fix.trim()
            },
        );
        if !incident.failed_fixes.is_empty() {
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "pitfalls.failed_fixes_prefix"),
                &incident.failed_fixes.join("; "),
            );
        }
        let first_observed = pitfall_first_observed(
            lang,
            &incident.first_observed_at,
            incident.timeline_complete,
            !incident.recent_observations.is_empty(),
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.first_observed_prefix"),
            &first_observed,
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.last_observed_prefix"),
            incident
                .last_observed_at
                .as_deref()
                .unwrap_or(legacy_missing),
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.last_recurred_prefix"),
            incident.last_recurred_at.as_deref().unwrap_or(not_recurred),
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.last_verified_prefix"),
            incident.last_verified_at.as_deref().unwrap_or(unverified),
        );
        let timeline = umadev_i18n::t(
            lang,
            if incident.timeline_complete {
                "pitfalls.timeline.complete"
            } else {
                "pitfalls.timeline.incomplete"
            },
        );
        let evidence = umadev_i18n::tf(
            lang,
            "pitfalls.evidence_value",
            &[&incident.recent_evidence_count.to_string(), timeline],
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "pitfalls.evidence_prefix"),
            &evidence,
        );
        push_pitfall_observations(&mut out, lang, &incident.recent_observations);
    }

    if !report.unclassified_candidates.is_empty() {
        out.push('\n');
        push_wrapped_pitfall_line(&mut out, umadev_i18n::t(lang, "pitfalls.candidates_header"));
        for candidate in &report.unclassified_candidates {
            out.push('\n');
            push_wrapped_pitfall_line(
                &mut out,
                &umadev_i18n::tf(
                    lang,
                    "pitfalls.candidate_item",
                    &[&candidate.fingerprint, &candidate.hits.to_string()],
                ),
            );
            let first_observed = pitfall_first_observed(
                lang,
                &candidate.first_observed_at,
                candidate.timeline_complete,
                !candidate.recent_observations.is_empty(),
            );
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "pitfalls.first_observed_prefix"),
                &first_observed,
            );
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "pitfalls.last_observed_prefix"),
                candidate
                    .last_observed_at
                    .as_deref()
                    .unwrap_or(legacy_missing),
            );
            let timeline = umadev_i18n::t(
                lang,
                if candidate.timeline_complete {
                    "pitfalls.timeline.complete"
                } else {
                    "pitfalls.timeline.incomplete"
                },
            );
            let evidence = umadev_i18n::tf(
                lang,
                "pitfalls.evidence_value",
                &[&candidate.recent_evidence_count.to_string(), timeline],
            );
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "pitfalls.evidence_prefix"),
                &evidence,
            );
            push_pitfall_observations(&mut out, lang, &candidate.recent_observations);
        }
    }
    out.trim_end().to_string()
}

/// Render the reusable-rule view for `/lessons`. Concrete incident rows
/// (`top_pitfalls`, `recurring`) and the legacy duplicate pattern list are
/// deliberately ignored; `/pitfalls` owns incidents, while successful patterns
/// already appear exactly once in `curated_lessons`.
pub(super) fn format_lessons_report(
    lang: umadev_i18n::Lang,
    report: &umadev_agent::LessonsReport,
) -> String {
    if report.is_empty() {
        let mut messages = Vec::new();
        if report.has_incidents() {
            messages.push(umadev_i18n::tf(
                lang,
                "lessons.incidents_pending",
                &[&report.efficacy.total.to_string()],
            ));
        }
        if report.has_unclassified_candidates() {
            messages.push(umadev_i18n::tf(
                lang,
                "lessons.candidates_pending",
                &[
                    &report.efficacy.unclassified_candidates.to_string(),
                    &report.efficacy.unclassified_candidate_hits.to_string(),
                ],
            ));
        }
        if messages.is_empty() {
            messages.push(umadev_i18n::t(lang, "lessons.empty").to_string());
        }
        return wrap_lesson_message(&messages.join("\n\n"));
    }

    let unknown = umadev_i18n::t(lang, "lessons.time.unknown");
    let legacy_missing = umadev_i18n::t(lang, "lessons.time.legacy_missing");
    let unverified = umadev_i18n::t(lang, "lessons.time.unverified");
    let mut out = String::new();
    for (index, lesson) in report.curated_lessons.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let status = umadev_i18n::t(lang, curated_lesson_status_key(lesson.status));
        let item_prefix = umadev_i18n::tf(
            lang,
            "lessons.item_prefix",
            &[&(index + 1).to_string(), status],
        );
        let title = if lesson.source_kind == "pitfall"
            && lesson
                .source_signatures
                .first()
                .is_some_and(|signature| signature == lesson.title.trim())
        {
            umadev_i18n::tf(lang, "lessons.pitfall_title", &[lesson.title.trim()])
        } else if lesson.title.trim().is_empty() {
            unknown.to_string()
        } else {
            lesson.title.trim().to_string()
        };
        push_lesson_field(&mut out, &item_prefix, &title);
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.rule_prefix"),
            if lesson.rule.trim().is_empty() {
                unknown
            } else {
                lesson.rule.trim()
            },
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.root_cause_prefix"),
            if lesson.root_cause.trim().is_empty() {
                unknown
            } else {
                lesson.root_cause.trim()
            },
        );
        let source = curated_lesson_source(lang, &lesson.source_kind);
        let evidence = umadev_i18n::tf(
            lang,
            "lessons.evidence_value",
            &[&lesson.evidence_count.to_string(), &source],
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.evidence_prefix"),
            &evidence,
        );
        if !lesson.source_signatures.is_empty() {
            push_lesson_field(
                &mut out,
                umadev_i18n::t(lang, "lessons.signatures_prefix"),
                &lesson.source_signatures.join(", "),
            );
        }
        let first_observed = if lesson.first_observed_at.trim().is_empty() {
            unknown.to_string()
        } else if !lesson.timeline_complete {
            umadev_i18n::tf(
                lang,
                "lessons.time.legacy_value",
                &[lesson.first_observed_at.trim()],
            )
        } else {
            lesson.first_observed_at.trim().to_string()
        };
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.first_observed_prefix"),
            &first_observed,
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.last_observed_prefix"),
            lesson.last_observed_at.as_deref().unwrap_or(legacy_missing),
        );
        push_lesson_field(
            &mut out,
            umadev_i18n::t(lang, "lessons.last_verified_prefix"),
            lesson.last_verified_at.as_deref().unwrap_or(unverified),
        );
    }
    out.trim_end().to_string()
}
