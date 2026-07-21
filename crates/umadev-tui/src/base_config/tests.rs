use super::{
    detect_base_context_window, detect_base_model, detect_base_reasoning, kimi_context_for_model,
    redetect_base_config,
};

#[test]
fn redetect_reads_the_base_config_live_not_a_cached_value() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".codex")).unwrap();
    std::fs::write(
        root.join(".codex/config.toml"),
        "model = \"gpt-5.6\"\nmodel_reasoning_effort = \"high\"\n",
    )
    .unwrap();

    let re = redetect_base_config("codex", root, "stale-cached-model");
    assert_eq!(re.model, "gpt-5.6");
    assert_eq!(re.reasoning.as_deref(), Some("high"));

    std::fs::write(
        root.join(".codex/config.toml"),
        "model = \"gpt-5.6\"\nmodel_reasoning_effort = \"xhigh\"\n",
    )
    .unwrap();
    let re = redetect_base_config("codex", root, "stale-cached-model");
    assert_eq!(
        re.reasoning.as_deref(),
        Some("xhigh"),
        "the changed effort is surfaced after re-establish"
    );
}

#[test]
fn redetect_is_fail_open_when_the_base_pins_no_model_or_effort() {
    let tmp = tempfile::TempDir::new().unwrap();
    let re = redetect_base_config("offline", tmp.path(), "fallback-model");
    assert_eq!(re.model, "fallback-model");
    assert_eq!(re.reasoning, None);
}

#[test]
fn reads_model_effort_and_context_from_grok_config_toml() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".grok")).unwrap();
    std::fs::write(
        root.join(".grok/config.toml"),
        "[models]\ndefault = \"grok-4.5\"\ndefault_reasoning_effort = \"high\"\n\n\
         [model.\"grok-4.5\"]\ncontext_window = 1000000\n",
    )
    .unwrap();

    assert_eq!(
        detect_base_model("grok-build", root).as_deref(),
        Some("grok-4.5")
    );
    assert_eq!(
        detect_base_reasoning("grok-build", root).as_deref(),
        Some("high")
    );
    assert_eq!(
        detect_base_context_window("grok-build", root),
        Some(1_000_000)
    );
}

#[test]
fn grok_config_with_only_mcp_servers_is_fail_open_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".grok")).unwrap();
    std::fs::write(
        root.join(".grok/config.toml"),
        "[mcp_servers.example]\ncommand = \"x\"\n",
    )
    .unwrap();
    assert_eq!(detect_base_model("grok-build", root), None);
    assert_eq!(detect_base_reasoning("grok-build", root), None);
    assert_eq!(detect_base_context_window("grok-build", root), None);
}

#[test]
fn context_uses_the_selected_kimi_alias_and_persistent_override() {
    let value: toml::Value = toml::from_str(
        r#"
default_model = "kimi-code/kimi-for-coding"

[models."kimi-code/kimi-for-coding"]
max_context_size = 262144

[models."kimi-code/kimi-for-coding".overrides]
max_context_size = 131072
"#,
    )
    .unwrap();
    assert_eq!(
        kimi_context_for_model(&value, "kimi-code/kimi-for-coding"),
        Some(131_072)
    );
    assert_eq!(kimi_context_for_model(&value, "missing"), None);
}
