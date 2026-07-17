//! Native background-process control for the resident base session.

use std::fmt::Write as _;
use std::sync::Arc;

use umadev_agent::{ChannelSink, EngineEvent, EventSink};
use umadev_runtime::{SessionCapability, SessionError};

use super::ChatSessionHolder;

#[derive(Debug)]
pub(super) enum BackgroundProcessRequest {
    List,
    Stop(String),
}

pub(super) fn spawn_background_process_control(
    holder: ChatSessionHolder,
    sink: Arc<ChannelSink>,
    lang: umadev_i18n::Lang,
    request: BackgroundProcessRequest,
) {
    let generation = holder.generation();
    tokio::spawn(async move {
        let mut guard = holder.lock().await;
        if holder.generation() != generation {
            sink.emit(EngineEvent::Note(
                umadev_i18n::t(lang, "processes.session_changed").to_string(),
            ));
            return;
        }
        let Some(resident) = guard.as_mut() else {
            sink.emit(EngineEvent::Note(
                umadev_i18n::t(lang, "processes.not_active").to_string(),
            ));
            return;
        };
        let result = match request {
            BackgroundProcessRequest::List => resident
                .session_mut()
                .list_background_processes()
                .await
                .map(|snapshot| render_background_process_snapshot(lang, &snapshot)),
            BackgroundProcessRequest::Stop(task_id) => resident
                .session_mut()
                .stop_background_process(&task_id)
                .await
                .map(|outcome| render_background_process_stop(lang, outcome)),
        };
        let note = match result {
            Ok(note) => note,
            Err(SessionError::CapabilityUnsupported(
                SessionCapability::BackgroundProcessControl,
            )) => umadev_i18n::t(lang, "processes.unsupported").to_string(),
            Err(error) => umadev_i18n::tf(
                lang,
                "processes.failed",
                &[&safe_background_process_text(&error.to_string())],
            ),
        };
        sink.emit(EngineEvent::Note(note));
    });
}

fn render_background_process_snapshot(
    lang: umadev_i18n::Lang,
    snapshot: &umadev_runtime::BackgroundProcessSnapshot,
) -> String {
    if snapshot.processes.is_empty() {
        return umadev_i18n::t(lang, "processes.empty").to_string();
    }
    let mut output = umadev_i18n::tf(
        lang,
        "processes.header",
        &[&snapshot.processes.len().to_string()],
    );
    for process in &snapshot.processes {
        let kind = match process.kind {
            umadev_runtime::BackgroundProcessKind::Bash => {
                umadev_i18n::t(lang, "processes.kind.bash")
            }
            umadev_runtime::BackgroundProcessKind::Monitor => {
                umadev_i18n::t(lang, "processes.kind.monitor")
            }
        };
        let status = if process.completed {
            process.exit_code.map_or_else(
                || {
                    process.signal.as_ref().map_or_else(
                        || umadev_i18n::t(lang, "processes.status.completed").to_string(),
                        |signal| {
                            umadev_i18n::tf(
                                lang,
                                "processes.status.signal",
                                &[&safe_background_process_text(signal)],
                            )
                        },
                    )
                },
                |code| umadev_i18n::tf(lang, "processes.status.exit", &[&code.to_string()]),
            )
        } else {
            umadev_i18n::t(lang, "processes.status.running").to_string()
        };
        let truncated = if process.truncated {
            umadev_i18n::t(lang, "processes.truncated")
        } else {
            ""
        };
        let task_id = safe_background_process_text(&process.task_id);
        let _ = write!(output, "\n- {task_id} · {kind} · {status}{truncated}");
    }
    output
}

fn render_background_process_stop(
    lang: umadev_i18n::Lang,
    outcome: umadev_runtime::BackgroundProcessStopOutcome,
) -> String {
    let key = match outcome {
        umadev_runtime::BackgroundProcessStopOutcome::Killed => "processes.stop.killed",
        umadev_runtime::BackgroundProcessStopOutcome::AlreadyExited => {
            "processes.stop.already_exited"
        }
        umadev_runtime::BackgroundProcessStopOutcome::NotFound => "processes.stop.not_found",
    };
    umadev_i18n::t(lang, key).to_string()
}

fn safe_background_process_text(value: &str) -> String {
    umadev_agent::base_error::strip_ansi(value)
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .take(512)
        .collect()
}
