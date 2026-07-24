use super::scope::{marker_matches_case_insensitive, parse_git_commit_paths};

pub(super) fn strip_git_commit_politeness(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start();
        let prefix_len = [
            "请你帮我",
            "請你幫我",
            "请帮我",
            "請幫我",
            "请你",
            "請你",
            "请",
            "請",
            "帮我",
            "幫我",
            "只",
            "仅",
            "僅",
            "麻烦",
            "麻煩",
            "请执行 ",
            "請執行 ",
            "执行 ",
            "執行 ",
            "执行",
            "執行",
            "直接",
            "现在",
            "現在",
            "please,",
            "please ",
            "now ",
            "go ahead and ",
        ]
        .iter()
        .find_map(|prefix| {
            marker_matches_case_insensitive(trimmed, prefix).then_some(prefix.len())
        });
        match prefix_len {
            Some(len) => text = &trimmed[len..],
            None => return trimmed,
        }
    }
}

pub(super) fn natural_git_commit_prefix(command: &str) -> Option<(usize, bool, bool)> {
    const ALL_DIRTY: &[&str] = &[
        "把这些变更提交",
        "把這些變更提交",
        "将这些变更提交",
        "將這些變更提交",
        "把当前改动提交",
        "把當前改動提交",
        "将当前改动提交",
        "將當前改動提交",
        "把当前变更提交",
        "把當前變更提交",
        "将当前变更提交",
        "將當前變更提交",
        "提交这些变更",
        "提交這些變更",
        "提交当前改动",
        "提交當前改動",
        "提交这些改动",
        "提交這些改動",
        "提交本次改动",
        "提交本次變動",
        "提交本次變更",
        "提交后总结",
        "提交後總結",
        "提交git记录",
        "提交git紀錄",
        "提交git纪录",
        "提交 git 记录",
        "提交 git 紀錄",
        "提交 git 纪录",
        "确认提交",
        "確認提交",
        "确定提交",
        "確定提交",
        "创建一个git提交",
        "建立一個git提交",
        "做一次git提交",
        "执行git提交",
        "執行git提交",
        "执行gitcommit",
        "執行gitcommit",
        "运行gitcommit",
        "運行gitcommit",
        "创建一个提交",
        "創建一個提交",
        "建立一个提交",
        "建立一個提交",
        "创建一次提交",
        "創建一次提交",
        "git提交",
    ];
    for prefix in ALL_DIRTY {
        if command.starts_with(prefix) {
            return Some((prefix.len(), false, false));
        }
    }
    for prefix in [
        "commit these changes",
        "commit all current changes",
        "commit all changes",
        "commit changes",
        "commit the changes",
        "commit current changes",
        "commit my changes",
        "commit this change",
        "commit now",
        "make one commit",
        "make a commit",
        "create one commit",
        "create a commit",
    ] {
        if let Some(tail) = command.strip_prefix(prefix) {
            if tail
                .chars()
                .next()
                .is_none_or(|first| !first.is_ascii_alphanumeric())
            {
                return Some((prefix.len(), false, false));
            }
        }
    }

    for prefix in ["提交这些文件", "提交這些文件", "提交文件"] {
        if command.starts_with(prefix) {
            return Some((prefix.len(), true, false));
        }
    }
    if let Some(tail) = command.strip_prefix("commit these files") {
        if tail
            .chars()
            .next()
            .is_none_or(|first| !first.is_ascii_alphanumeric())
        {
            return Some(("commit these files".len(), true, false));
        }
    }
    for prefix in ["提交", "commit "] {
        if command.starts_with(prefix) {
            return Some((prefix.len(), true, true));
        }
    }
    None
}

pub(super) fn trim_natural_commit_tail(mut tail: &str) -> &str {
    tail = tail.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | '、' | ':' | '：')
    });
    tail = tail.trim_end_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | '、' | ':' | '：' | '.' | '。' | '!' | '！')
    });
    if let Some(index) = find_safe_git_receipt_suffix(tail) {
        tail = &tail[..index];
    }
    tail = tail.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | '、' | ':' | '：')
    });
    tail = tail.trim_end_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | '、' | ':' | '：' | '.' | '。' | '!' | '！')
    });
    for suffix in [
        "到 git 仓库",
        "到 git 倉庫",
        "到git仓库",
        "到git倉庫",
        "进 git 仓库",
        "進 git 倉庫",
        "进git仓库",
        "進git倉庫",
        "一下",
        "吧",
        " now",
    ] {
        let lower = tail.to_lowercase();
        if lower.ends_with(suffix) {
            tail = tail[..tail.len() - suffix.len()]
                .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '，' | '、'));
        }
    }
    tail
}

/// Whether an all-dirty commit request is followed only by a scope-narrowing
/// modifier, rather than another piece of work. This deliberately accepts a
/// small exact language: unknown prose remains compound/invalid and is never
/// swallowed into the host-owned commit lane.
pub(super) fn natural_git_commit_tail_is_safe_constraint(tail: &str) -> bool {
    let trimmed = tail.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，' | '、' | ';' | '；' | ':' | '：' | '.' | '。' | '!' | '！'
            )
    });
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_lowercase();
    if [
        "即可",
        "就行",
        "就好",
        "就可以",
        "就可以了",
        "而已",
        "only",
        "only commit",
        "commit only",
    ]
    .contains(&lower.as_str())
    {
        return true;
    }

    let clauses = lower.split([',', '，', '、', ';', '；', '/', '／']);
    let mut saw_clause = false;
    for raw in clauses {
        let mut clause = raw.trim();
        while let Some(rest) = [
            "并且", "並且", "并", "並", "同时", "同時", "然后", "然後", "and ",
        ]
        .iter()
        .find_map(|prefix| clause.strip_prefix(prefix))
        {
            clause = rest.trim_start();
        }
        if clause.is_empty() {
            continue;
        }
        saw_clause = true;
        if ![
            "即可",
            "就行",
            "就好",
            "就可以",
            "就可以了",
            "而已",
            "only",
            "only commit",
            "commit only",
            "不要跑评审",
            "不要跑評審",
            "不要评审",
            "不要評審",
            "不要启动评审",
            "不要啟動評審",
            "不要团队评审",
            "不要團隊評審",
            "不要跑qc",
            "不要运行qc",
            "不要運行qc",
            "不要修改代码",
            "不要修改代碼",
            "不要改代码",
            "不要改代碼",
            "不要修改文件",
            "不要改文件",
            "不要做其他事情",
            "不要做其它事情",
            "不要做额外工作",
            "不要做額外工作",
            "不要运行测试",
            "不要運行測試",
            "不要跑测试",
            "不要跑測試",
            "别跑评审",
            "別跑評審",
            "别评审",
            "別評審",
            "别改代码",
            "別改代碼",
            "别改文件",
            "別改文件",
            "do not review",
            "don't review",
            "dont review",
            "do not run reviews",
            "don't run reviews",
            "dont run reviews",
            "do not modify code",
            "don't modify code",
            "dont modify code",
            "do not edit files",
            "don't edit files",
            "dont edit files",
            "do nothing else",
        ]
        .contains(&clause)
        {
            return false;
        }
    }
    saw_clause
}

pub(super) fn find_safe_git_receipt_suffix(text: &str) -> Option<usize> {
    const STARTS: &[&str] = &[
        "然后", "然後", "后", "後", "并", "並", "接着", "接著", "and ", "then ",
    ];
    const MAX_RECEIPT_SUFFIX_BYTES: usize = 512;
    let mut window_start = text.len().saturating_sub(MAX_RECEIPT_SUFFIX_BYTES);
    while window_start < text.len() && !text.is_char_boundary(window_start) {
        window_start += 1;
    }
    let mut earliest = None;
    for (relative_index, _) in text[window_start..].char_indices() {
        let index = window_start + relative_index;
        for marker in STARTS {
            if marker_matches_case_insensitive(&text[index..], marker)
                && git_receipt_suffix_is_safe(&text[index..])
            {
                earliest = Some(earliest.map_or(index, |current: usize| current.min(index)));
            }
        }
    }
    earliest
}

fn git_receipt_suffix_is_safe(text: &str) -> bool {
    let trimmed = text.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，' | '、' | ';' | '；' | ':' | '：' | '.' | '。' | '!' | '！'
            )
    });
    let lower = trimmed.to_lowercase();
    let english = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    if [
        "and tell me the hash",
        "then tell me the hash",
        "and tell me the commit hash",
        "then tell me the commit hash",
        "and show me the hash",
        "then show me the hash",
        "and show me the commit hash",
        "then show me the commit hash",
        "and report the hash",
        "then report the hash",
        "and report the commit hash",
        "then report the commit hash",
        "and give me the hash",
        "then give me the hash",
        "and summarize the commit",
        "then summarize the commit",
        "and summarise the commit",
        "then summarise the commit",
        "and summarize this commit",
        "then summarize this commit",
        "and summarize the committed changes",
        "then summarize the committed changes",
        "and give me a summary of the commit",
        "then give me a summary of the commit",
        "and report the commit result",
        "then report the commit result",
    ]
    .contains(&english.as_str())
    {
        return true;
    }

    let compact: String = lower
        .chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    ',' | '，' | '、' | ';' | '；' | ':' | '：' | '.' | '。' | '!' | '！'
                )
        })
        .collect();
    [
        "然后告诉我hash",
        "然後告訴我hash",
        "后告诉我hash",
        "後告訴我hash",
        "并告诉我hash",
        "並告訴我hash",
        "接着告诉我hash",
        "接著告訴我hash",
        "然后告诉我哈希",
        "然後告訴我哈希",
        "后告诉我哈希",
        "後告訴我哈希",
        "并告诉我哈希",
        "並告訴我哈希",
        "接着告诉我哈希",
        "接著告訴我哈希",
        "然后告诉我提交哈希",
        "然後告訴我提交哈希",
        "后告诉我提交哈希",
        "後告訴我提交哈希",
        "并告诉我提交哈希",
        "並告訴我提交哈希",
        "接着告诉我提交哈希",
        "接著告訴我提交哈希",
        "然后给我hash",
        "然後給我hash",
        "后给我hash",
        "後給我hash",
        "然后给我哈希",
        "然後給我哈希",
        "后给我哈希",
        "後給我哈希",
        "后总结",
        "後總結",
        "然后总结本次提交",
        "然後總結本次提交",
        "然后总结这次提交",
        "然後總結這次提交",
        "后总结本次提交",
        "後總結本次提交",
        "后汇报提交结果",
        "後匯報提交結果",
        "然后汇报提交结果",
        "然後匯報提交結果",
        "并返回hash",
        "並返回hash",
        "然后返回hash",
        "然後返回hash",
        "并返回哈希",
        "並返回哈希",
        "然后返回哈希",
        "然後返回哈希",
        "并返回提交哈希",
        "並返回提交哈希",
        "然后返回提交哈希",
        "然後返回提交哈希",
    ]
    .contains(&compact.as_str())
}

pub(super) fn parse_paths(tail: &str) -> Option<Vec<String>> {
    parse_git_commit_paths(tail)
}
