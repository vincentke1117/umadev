use super::{extension_of, Decision};

/// **UG-LINT-008**: ban `var` declarations (use `let`/`const`).
///
/// `var` has function-scoped hoisting and can leak loop variables. The rule is
/// conservative and only flags more than two declarations. An ES5 launcher may
/// use the exact file-header directive
/// `// umadev-governance: allow-es5-bootstrap`.
#[must_use]
pub fn check_var_declarations(file_path: &str, content: &str) -> Decision {
    let ext = extension_of(file_path);
    if !matches!(ext.as_str(), "ts" | "js" | "jsx" | "tsx")
        || crate::tokenizer::allows_es5_bootstrap(content)
    {
        return Decision::pass();
    }
    if file_path.contains(".test.") || file_path.contains(".spec.") {
        return Decision::pass();
    }
    let hits = content
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with("//") && !line.starts_with('*'))
        .filter(|line| line.starts_with("var ") || line.starts_with("var\t"))
        .count();
    if hits > 2 {
        Decision::block(
            "UG-LINT-008",
            format!(
                "UmaDev: var declarations banned (UG-LINT-008). \
                 `{file_path}` has {hits} `var` declarations — `var` has \
                 function-scoped hoisting causing subtle bugs. Use `const` for \
                 values that never change, and `let` for reassignable variables. \
                 Both are block-scoped.",
            ),
        )
    } else {
        Decision::pass()
    }
}
