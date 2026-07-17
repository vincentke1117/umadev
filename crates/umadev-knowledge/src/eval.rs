//! Deterministic retrieval-quality metrics for release gates.

use std::collections::BTreeSet;

/// One query's expected relevant path fragments and observed ranked paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalJudgment {
    /// Stable fixture identifier.
    pub id: String,
    /// Path fragments accepted as relevant for this query.
    pub relevant: Vec<String>,
    /// Retrieved paths in descending rank order.
    pub ranked: Vec<String>,
}

/// Aggregate binary-relevance metrics at a fixed cutoff.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalEvalReport {
    /// Rank cutoff used by every metric.
    pub k: usize,
    /// Mean fraction of relevant judgments found within the cutoff.
    pub recall_at_k: f64,
    /// Mean reciprocal rank of the first relevant result.
    pub mrr: f64,
    /// Mean normalized discounted cumulative gain at the cutoff.
    pub ndcg_at_k: f64,
    /// Fixture ids with no relevant result inside the cutoff.
    pub misses: Vec<String>,
}

/// One query for which a mature retriever should return no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbstentionJudgment {
    /// Stable fixture identifier.
    pub id: String,
    /// Retrieved paths in descending rank order; empty means the retriever
    /// correctly abstained.
    pub ranked: Vec<String>,
}

/// Aggregate quality of no-answer behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstentionEvalReport {
    /// Number of negative queries evaluated.
    pub cases: usize,
    /// Fraction of negative queries that produced no results.
    pub accuracy: f64,
    /// Fixture ids that produced at least one false-positive result.
    pub false_positives: Vec<String>,
}

/// Evaluate ranked paths using case-insensitive path-fragment relevance.
#[must_use]
pub fn evaluate_rankings(judgments: &[RetrievalJudgment], k: usize) -> RetrievalEvalReport {
    if judgments.is_empty() || k == 0 {
        return RetrievalEvalReport {
            k,
            recall_at_k: 0.0,
            mrr: 0.0,
            ndcg_at_k: 0.0,
            misses: judgments.iter().map(|case| case.id.clone()).collect(),
        };
    }

    let mut recall = 0.0;
    let mut reciprocal_rank = 0.0;
    let mut ndcg = 0.0;
    let mut misses = Vec::new();
    for case in judgments {
        let relevant = case
            .relevant
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if relevant.is_empty() {
            misses.push(case.id.clone());
            continue;
        }
        let mut matched = BTreeSet::new();
        let mut first_rank = None;
        let mut dcg = 0.0;
        for (index, path) in case.ranked.iter().take(k).enumerate() {
            let path = path.to_ascii_lowercase();
            let Some(label) = relevant
                .iter()
                .find(|needle| path.contains(needle.as_str()))
            else {
                continue;
            };
            if !matched.insert(label.clone()) {
                continue;
            }
            let rank = index + 1;
            first_rank.get_or_insert(rank);
            dcg += 1.0 / ((rank as f64) + 1.0).log2();
        }
        recall += matched.len() as f64 / relevant.len() as f64;
        if let Some(rank) = first_rank {
            reciprocal_rank += 1.0 / rank as f64;
        } else {
            misses.push(case.id.clone());
        }
        let ideal_hits = relevant.len().min(k);
        let ideal_dcg = (1..=ideal_hits)
            .map(|rank| 1.0 / ((rank as f64) + 1.0).log2())
            .sum::<f64>();
        if ideal_dcg > 0.0 {
            ndcg += dcg / ideal_dcg;
        }
    }
    let count = judgments.len() as f64;
    RetrievalEvalReport {
        k,
        recall_at_k: recall / count,
        mrr: reciprocal_rank / count,
        ndcg_at_k: ndcg / count,
        misses,
    }
}

/// Evaluate whether negative/no-answer queries correctly produce no results.
#[must_use]
pub fn evaluate_abstentions(judgments: &[AbstentionJudgment]) -> AbstentionEvalReport {
    if judgments.is_empty() {
        return AbstentionEvalReport {
            cases: 0,
            accuracy: 0.0,
            false_positives: Vec::new(),
        };
    }
    let false_positives = judgments
        .iter()
        .filter(|case| !case.ranked.is_empty())
        .map(|case| case.id.clone())
        .collect::<Vec<_>>();
    let correct = judgments.len().saturating_sub(false_positives.len());
    AbstentionEvalReport {
        cases: judgments.len(),
        accuracy: correct as f64 / judgments.len() as f64,
        false_positives,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_recall_mrr_and_ndcg_without_double_counting_one_label() {
        let report = evaluate_rankings(
            &[RetrievalJudgment {
                id: "auth".into(),
                relevant: vec!["auth.md".into(), "session.md".into()],
                ranked: vec![
                    "other.md".into(),
                    "auth.md#one".into(),
                    "auth.md#two".into(),
                    "session.md".into(),
                ],
            }],
            4,
        );
        assert!((report.recall_at_k - 1.0).abs() < f64::EPSILON);
        assert!((report.mrr - 0.5).abs() < f64::EPSILON);
        assert!(report.ndcg_at_k > 0.5 && report.ndcg_at_k < 1.0);
        assert!(report.misses.is_empty());
    }

    #[test]
    fn empty_or_zero_cutoff_is_an_explicit_miss() {
        let case = RetrievalJudgment {
            id: "q1".into(),
            relevant: vec!["a".into()],
            ranked: vec!["a".into()],
        };
        let report = evaluate_rankings(&[case], 0);
        assert_eq!(report.misses, ["q1"]);
        assert!(report.recall_at_k.abs() < f64::EPSILON);
    }

    #[test]
    fn evaluates_no_answer_cases_separately_from_positive_recall() {
        let report = evaluate_abstentions(&[
            AbstentionJudgment {
                id: "clean".into(),
                ranked: Vec::new(),
            },
            AbstentionJudgment {
                id: "false-positive".into(),
                ranked: vec!["unrelated.md".into()],
            },
        ]);
        assert_eq!(report.cases, 2);
        assert!((report.accuracy - 0.5).abs() < f64::EPSILON);
        assert_eq!(report.false_positives, ["false-positive"]);
    }
}
