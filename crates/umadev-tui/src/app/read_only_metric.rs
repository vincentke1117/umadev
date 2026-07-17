//! Honest summaries for low-signal read-only tool output.

/// Fold a read-only tool's raw result into a single metric instead of dumping
/// it. Grep output may contain arbitrary project data, so only a provider's
/// explicit count phrase is safe to label as a count.
pub(super) fn read_only_metric(
    lang: umadev_i18n::Lang,
    name: &str,
    preview: &str,
) -> Option<String> {
    let n = explicit_read_only_count(preview);
    match (name, n) {
        ("Grep" | "Glob", Some(n)) => {
            Some(umadev_i18n::tf(lang, "tui.tool.matches", &[&n.to_string()]))
        }
        _ => None,
    }
}

/// Extract a provider-declared count from the first non-empty summary line.
/// The grammar stays deliberately narrow because every other line may be a
/// verbatim match from the user's repository.
fn explicit_read_only_count(preview: &str) -> Option<usize> {
    let line = preview.lines().find(|line| !line.trim().is_empty())?;
    let words: Vec<String> = line
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| {
                !c.is_ascii_alphanumeric()
                    && !matches!(c, '处' | '處' | '个' | '個' | '匹' | '配' | '文' | '件')
            })
            .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();

    let (count_at, unit_at) = match words.first().map(String::as_str) {
        Some("found" | "matched" | "matches" | "total" | "count" | "共" | "找到") => (1, 2),
        Some(_) => (0, 1),
        None => return None,
    };
    let count = words.get(count_at)?.parse::<usize>().ok()?;
    let unit = words.get(unit_at)?.as_str();
    matches!(
        unit,
        "match"
            | "matches"
            | "result"
            | "results"
            | "file"
            | "files"
            | "path"
            | "paths"
            | "line"
            | "lines"
            | "处匹配"
            | "處匹配"
            | "个匹配"
            | "個匹配"
            | "个文件"
            | "個文件"
    )
    .then_some(count)
}
