//! Small, deterministic bilingual query expansion for the offline retriever.
//!
//! BM25 cannot connect a Chinese request to an English-only document (or the
//! reverse) when the two share no literal token. The optional vector channel
//! handles that case when a local model is installed; this bounded alias layer
//! keeps the default offline path useful too. It expands only explicit product
//! concepts and is rank-fused with the untouched query, so literal evidence
//! remains the stronger signal.

use std::collections::BTreeSet;

const MAX_EXPANSION_CHARS: usize = 1_024;

const CONCEPTS: &[&[&str]] = &[
    &[
        "authentication",
        "authorization",
        "auth",
        "login",
        "sign in",
        "登录",
        "登入",
        "鉴权",
        "认证",
        "身份验证",
    ],
    &["session", "会话", "登录态"],
    &["credential", "credentials", "凭证", "密钥"],
    &["rotation", "rotate", "轮换", "滚动更新"],
    &["idempotency", "idempotent", "幂等", "防重复"],
    &["retry", "retries", "重试", "再次尝试"],
    &["rollback", "roll back", "回滚", "版本回退"],
    &["accessibility", "accessible", "wcag", "无障碍", "辅助功能"],
    &[
        "internationalization",
        "localization",
        "i18n",
        "l10n",
        "国际化",
        "本地化",
    ],
    &["observability", "telemetry", "可观测性", "监控追踪"],
    &["right to erasure", "erasure", "被遗忘权", "数据删除"],
    &["retention", "保留期限", "数据保留"],
    &["encoding", "mojibake", "编码", "乱码"],
    &["migration", "migrate", "迁移", "数据库变更"],
    &["flaky test", "flakiness", "偶发测试", "不稳定测试"],
    &["supply chain", "software supply chain", "软件供应链"],
    &["memory", "long term memory", "长期记忆", "记忆"],
    &["pitfall", "pitfalls", "踩坑", "故障经验"],
];

fn ascii_alias_matches(
    alias: &str,
    ascii_terms: &BTreeSet<String>,
    ascii_sequence: &[String],
) -> bool {
    let phrase = alias.split_ascii_whitespace().collect::<Vec<_>>();
    if phrase.len() == 1 {
        return ascii_terms.contains(alias);
    }
    ascii_sequence
        .windows(phrase.len())
        .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
}

/// Append aliases for product concepts explicitly present in `query`.
///
/// The original query is always the leading prefix. If no concept matches, the
/// returned string is byte-for-byte equal to the input. Expansion is bounded
/// and deterministic; it performs no language detection, I/O, or network call.
#[must_use]
pub fn expand_bilingual_query(query: &str) -> String {
    let lower = query.to_ascii_lowercase();
    let ascii_sequence = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let ascii_terms = ascii_sequence.iter().cloned().collect::<BTreeSet<_>>();
    let mut additions = BTreeSet::new();
    for concept in CONCEPTS {
        let matched = concept.iter().any(|alias| {
            if alias.is_ascii() {
                ascii_alias_matches(alias, &ascii_terms, &ascii_sequence)
            } else {
                query.contains(alias)
            }
        });
        if matched {
            additions.extend(concept.iter().map(|alias| (*alias).to_string()));
        }
    }
    if additions.is_empty() {
        return query.to_string();
    }

    let mut out = query.to_string();
    let original_chars = out.chars().count();
    for alias in additions {
        let separator = usize::from(!out.is_empty());
        if out
            .chars()
            .count()
            .saturating_sub(original_chars)
            .saturating_add(separator)
            .saturating_add(alias.chars().count())
            > MAX_EXPANSION_CHARS
        {
            break;
        }
        if separator == 1 {
            out.push(' ');
        }
        out.push_str(&alias);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_is_identity_for_an_unknown_query() {
        let query = "draw a purple triangle";
        assert_eq!(expand_bilingual_query(query), query);
    }

    #[test]
    fn expands_both_language_directions() {
        let chinese = expand_bilingual_query("登录凭证轮换");
        assert!(chinese.contains("authentication"));
        assert!(chinese.contains("credential"));
        assert!(chinese.contains("rotation"));

        let english = expand_bilingual_query("idempotent retry");
        assert!(english.contains("幂等"));
        assert!(english.contains("重试"));
    }

    #[test]
    fn short_ascii_substrings_do_not_trigger_aliases() {
        for query in ["author biography", "design interface"] {
            assert_eq!(expand_bilingual_query(query), query);
        }
    }
}
