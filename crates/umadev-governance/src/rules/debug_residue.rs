//! Production debug-residue analysis for `UD-ARCH-002`.

use std::collections::HashSet;

use super::{extension_of, looks_like_secret_test_path, strip_string_literals, Decision};

const SCANNED_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "py", "rb", "go", "rs", "java", "kt", "swift", "php", "vue", "svelte",
];

struct Pattern {
    trigger: &'static str,
    label: &'static str,
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        trigger: "console.log",
        label: "console.log",
    },
    Pattern {
        trigger: "console.debug",
        label: "console.debug",
    },
    Pattern {
        trigger: "console.trace",
        label: "console.trace",
    },
    Pattern {
        trigger: "debugger;",
        label: "debugger",
    },
    Pattern {
        trigger: "debugger ",
        label: "debugger",
    },
    Pattern {
        trigger: "print(\"",
        label: "print(\"...\")",
    },
    Pattern {
        trigger: "print(f\"",
        label: "print(f\"...\")",
    },
];

/// Ban leftover debug statements in shipping source while allowing tests,
/// scripts, CLI output, comments, diagnostics, and guarded development logs.
#[must_use]
pub fn check_debug_residue(file_path: &str, content: &str) -> Decision {
    let ext = extension_of(file_path);
    let lower_path = file_path.to_ascii_lowercase();
    if !SCANNED_EXTENSIONS.contains(&ext.as_str())
        || looks_like_secret_test_path(file_path)
        || lower_path.contains("/scripts/")
        || lower_path.contains("/bin/")
    {
        return Decision::pass();
    }

    let mut hits = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with('*')
            || trimmed.starts_with("/*")
        {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("if (debug") || lower.contains("if(debug") || lower.contains("if (__dev")
        {
            continue;
        }
        let no_strings = strip_string_literals(line);
        for pattern in PATTERNS {
            let haystack = if pattern.trigger.starts_with("print") {
                if ext != "py" || !lower.contains("debug") {
                    continue;
                }
                line
            } else {
                &no_strings
            };
            if haystack.contains(pattern.trigger) {
                hits.push(pattern.label);
            }
        }
    }
    if hits.is_empty() {
        return Decision::pass();
    }

    let labels: Vec<_> = hits
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    Decision::block(
        "UD-ARCH-002",
        format!(
            "UmaDev: debug residue in source (UD-ARCH-002). `{file_path}` contains leftover {} \
             statement{} ({} hit{}). Remove debug output before shipping — it can log secrets \
             and bloats the bundle. Keep it behind a `if (DEBUG)` guard or use a logger that \
             respects `NODE_ENV`.",
            labels.join(" / "),
            if labels.len() == 1 { "" } else { "s" },
            hits.len(),
            if hits.len() == 1 { "" } else { "s" },
        ),
    )
}
