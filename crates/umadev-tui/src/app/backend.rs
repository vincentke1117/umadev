//! Backend-derived picker and command-palette data.

use super::{AuthMark, BackendInfo, PickerGroup, PickerItem, PickerStep};

/// Sentinel that prefixes the structured auth metadata a backend probe packs
/// into its human-readable detail field.
pub(crate) const PROBE_AUTH_SENTINEL: char = '\u{1}';

fn backend_label(lang: umadev_i18n::Lang, id: &str) -> String {
    let key = match id {
        "claude-code" => "backend.claude",
        "codex" => "backend.codex",
        "opencode" => "backend.opencode",
        "grok-build" => "backend.grok",
        "kimi-code" => "backend.kimi",
        // Fail-open for an impossible internal mismatch; the caller only iterates
        // the fixed five-id product list below.
        _ => return id.to_string(),
    };
    umadev_i18n::t(lang, key).to_string()
}

/// Build the options for one first-run picker step. Live probe data supplies
/// each backend's readiness, authentication, and remediation details.
pub(super) fn step_items(
    step: PickerStep,
    lang: umadev_i18n::Lang,
    backends: &[BackendInfo],
) -> Vec<PickerItem> {
    match step {
        PickerStep::Language => umadev_i18n::Lang::ALL
            .iter()
            .map(|&lang| PickerItem {
                backend_id: None,
                label: lang.label().to_string(),
                ready: true,
                detail: lang.code().to_string(),
                group: PickerGroup::Language,
                lang: Some(lang),
                auth: AuthMark::LoggedIn,
                login_cmd: String::new(),
                install_cmd: String::new(),
            })
            .collect(),
        PickerStep::BaseCli => crate::FIRST_CLASS_BACKEND_IDS
            .iter()
            .map(|id| {
                let display = backend_label(lang, id);
                let probe = backends.iter().find(|backend| backend.id == *id);
                PickerItem {
                    backend_id: Some((*id).to_string()),
                    label: display,
                    ready: probe.is_some_and(|backend| backend.ready),
                    detail: probe.map_or_else(
                        || "detecting...".to_string(),
                        |backend| backend.detail.clone(),
                    ),
                    group: PickerGroup::HostCli,
                    lang: None,
                    auth: probe.map_or(AuthMark::Unknown, |backend| backend.auth),
                    login_cmd: probe
                        .map(|backend| backend.login_cmd.clone())
                        .unwrap_or_default(),
                    install_cmd: probe
                        .map(|backend| backend.install_cmd.clone())
                        .unwrap_or_default(),
                }
            })
            .collect(),
    }
}

/// Unpack the auth tag `spawn_probe` packed onto a probe `detail`. Returns
/// `(auth_mark, login_cmd, install_cmd, human_detail)`. **Fail-open**: a `detail`
/// with no sentinel (an external emitter, an older build) yields
/// `(Unknown, "", "", detail)`.
pub(crate) fn parse_probe_detail(detail: &str) -> (AuthMark, String, String, String) {
    let Some(rest) = detail.strip_prefix(PROBE_AUTH_SENTINEL) else {
        return (
            AuthMark::Unknown,
            String::new(),
            String::new(),
            detail.to_string(),
        );
    };
    let Some((meta, human)) = rest.split_once(PROBE_AUTH_SENTINEL) else {
        return (
            AuthMark::Unknown,
            String::new(),
            String::new(),
            rest.to_string(),
        );
    };
    let mut auth = AuthMark::Unknown;
    let mut login = String::new();
    let mut install = String::new();
    for field in meta.split('|') {
        if let Some(value) = field.strip_prefix("auth=") {
            auth = AuthMark::from_tag(value);
        } else if let Some(value) = field.strip_prefix("login=") {
            login = value.to_string();
        } else if let Some(value) = field.strip_prefix("install=") {
            install = value.to_string();
        }
    }
    (auth, login, install, human.to_string())
}

pub(super) fn refresh_picker_with_probes(items: &mut [PickerItem], probes: &[BackendInfo]) {
    for item in items.iter_mut() {
        if let Some(id) = item.backend_id.as_deref() {
            if let Some(probe) = probes.iter().find(|probe| probe.id == id) {
                item.ready = probe.ready;
                item.detail.clone_from(&probe.detail);
                item.auth = probe.auth;
                item.login_cmd.clone_from(&probe.login_cmd);
                item.install_cmd.clone_from(&probe.install_cmd);
            }
        }
    }
}
