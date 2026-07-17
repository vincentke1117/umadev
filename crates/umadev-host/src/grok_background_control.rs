//! Pinned Grok Build background-process list/stop wire contract.
//!
//! The audited Grok source exposes logical ACP extensions
//! `x.ai/task/list` and `x.ai/task/kill`. ACP transports extension requests
//! with a leading underscore, so UmaDev writes `_x.ai/task/list` and
//! `_x.ai/task/kill`. Both responses are `ExtMethodResult` envelopes: the
//! operation payload is nested under `result`.

use std::collections::HashSet;

use serde_json::{json, Value};
use umadev_runtime::{
    BackgroundProcessKind, BackgroundProcessSnapshot, BackgroundProcessSnapshotEntry,
    BackgroundProcessStopOutcome,
};

/// Raw ACP method for the pinned source's background-process list extension.
pub const GROK_TASK_LIST_METHOD: &str = "_x.ai/task/list";
/// Raw ACP method for the pinned source's background-process stop extension.
pub const GROK_TASK_KILL_METHOD: &str = "_x.ai/task/kill";

const MAX_TASKS: usize = 1_024;
const MAX_ID_CHARS: usize = 512;
const MAX_SIGNAL_CHARS: usize = 256;

/// Build the exact request parameters for `x.ai/task/list`.
#[must_use]
pub fn list_params(session_id: &str) -> Value {
    json!({"sessionId":session_id})
}

/// Build the exact request parameters for `x.ai/task/kill`.
#[must_use]
pub fn kill_params(session_id: &str, task_id: &str) -> Value {
    json!({"sessionId":session_id,"taskId":task_id})
}

/// Parse and session-scope the pinned source's `x.ai/task/list` response.
///
/// Grok's terminal backend is shared with subagents, so its native list may
/// contain parent, sibling, or child tasks. Only records whose
/// `owner_session_id` exactly equals `expected_session_id` are returned.
/// Ownerless legacy records are intentionally excluded: this audited source
/// stamps owners, and an ownerless record is not safe evidence for a destructive
/// stop operation.
pub fn parse_list_response(
    response: &Value,
    expected_session_id: &str,
) -> Result<BackgroundProcessSnapshot, &'static str> {
    if !valid_id(expected_session_id) {
        return Err("invalid active session id");
    }
    let result = response
        .get("result")
        .and_then(Value::as_object)
        .ok_or("Grok task list omitted its result envelope")?;
    let tasks = result
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or("Grok task list omitted its tasks array")?;
    if tasks.len() > MAX_TASKS {
        return Err("Grok task list exceeded the bounded task limit");
    }

    let mut seen = HashSet::new();
    let mut processes = Vec::new();
    for task in tasks {
        let object = task
            .as_object()
            .ok_or("Grok task list contained a non-object task")?;
        let task_id = object
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
            .ok_or("Grok task list contained an invalid task id")?;
        let owner = match object.get("owner_session_id") {
            Some(Value::String(owner)) if valid_id(owner) => owner.as_str(),
            None | Some(Value::Null) => continue,
            _ => return Err("Grok task list contained an invalid owner id"),
        };
        if owner != expected_session_id {
            continue;
        }
        if !seen.insert(task_id.to_string()) {
            return Err("Grok task list contained a duplicate owned task id");
        }

        let completed = object
            .get("completed")
            .and_then(Value::as_bool)
            .ok_or("Grok task list omitted task completion state")?;
        let truncated = object
            .get("truncated")
            .and_then(Value::as_bool)
            .ok_or("Grok task list omitted task truncation state")?;
        let kind = match object.get("kind").and_then(Value::as_str).unwrap_or("bash") {
            "bash" => BackgroundProcessKind::Bash,
            "monitor" => BackgroundProcessKind::Monitor,
            _ => return Err("Grok task list contained an unknown task kind"),
        };
        let exit_code = optional_i32(object.get("exit_code"))?;
        let signal = optional_bounded_signal(object.get("signal"))?;
        if !completed && (exit_code.is_some() || signal.is_some()) {
            return Err("Grok running task carried terminal status");
        }
        processes.push(BackgroundProcessSnapshotEntry {
            task_id: task_id.to_string(),
            kind,
            completed,
            exit_code,
            signal,
            truncated,
        });
    }
    processes.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    Ok(BackgroundProcessSnapshot {
        session_id: expected_session_id.to_string(),
        processes,
    })
}

/// Parse the exact `ExtMethodResult<KillTaskResponse>` success envelope.
pub fn parse_kill_response(
    response: &Value,
    expected_task_id: &str,
) -> Result<BackgroundProcessStopOutcome, &'static str> {
    if !valid_id(expected_task_id) {
        return Err("invalid background task id");
    }
    let result = response
        .get("result")
        .and_then(Value::as_object)
        .ok_or("Grok task kill omitted its result envelope")?;
    let task_id = result
        .get("taskId")
        .and_then(Value::as_str)
        .filter(|value| valid_id(value))
        .ok_or("Grok task kill omitted a valid task id")?;
    if task_id != expected_task_id {
        return Err("Grok task kill response did not match the requested task");
    }
    match result.get("outcome").and_then(Value::as_str) {
        Some("killed") => Ok(BackgroundProcessStopOutcome::Killed),
        Some("already_exited") => Ok(BackgroundProcessStopOutcome::AlreadyExited),
        Some("not_found") => Ok(BackgroundProcessStopOutcome::NotFound),
        _ => Err("Grok task kill returned an unknown outcome"),
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= MAX_ID_CHARS
        && !value.chars().any(char::is_control)
}

fn optional_i32(value: Option<&Value>) -> Result<Option<i32>, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or("Grok task list contained an invalid exit code"),
    }
}

fn optional_bounded_signal(value: Option<&Value>) -> Result<Option<String>, &'static str> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if value.chars().count() <= MAX_SIGNAL_CHARS
                && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value.clone()))
        }
        _ => Err("Grok task list contained an invalid signal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(task_id: &str, owner: Option<&str>, completed: bool) -> Value {
        json!({
            "task_id":task_id,
            "command":"private and deliberately ignored",
            "cwd":"/private/ignored",
            "start_time":{"secs_since_epoch":1,"nanos_since_epoch":0},
            "end_time":if completed { json!({"secs_since_epoch":2,"nanos_since_epoch":0}) } else { Value::Null },
            "output":"private and deliberately ignored",
            "output_file":"/private/ignored.log",
            "truncated":false,
            "exit_code":if completed { json!(0) } else { Value::Null },
            "signal":Value::Null,
            "completed":completed,
            "kind":"bash",
            "block_waited":false,
            "explicitly_killed":false,
            "owner_session_id":owner,
        })
    }

    #[test]
    fn list_is_exactly_scoped_sorted_and_path_free() {
        let response = json!({"result":{"tasks":[
            task("own-b", Some("session-1"), false),
            task("foreign", Some("session-2"), false),
            task("legacy", None, false),
            task("own-a", Some("session-1"), true)
        ]}});
        let snapshot = parse_list_response(&response, "session-1").unwrap();
        assert_eq!(snapshot.session_id, "session-1");
        assert_eq!(
            snapshot
                .processes
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            ["own-a", "own-b"]
        );
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("command"));
        assert!(!encoded.contains("output"));
    }

    #[test]
    fn list_rejects_duplicate_owned_ids_unknown_kinds_and_over_limit() {
        let duplicate = json!({"result":{"tasks":[
            task("same", Some("session-1"), false),
            task("same", Some("session-1"), false)
        ]}});
        assert!(parse_list_response(&duplicate, "session-1").is_err());

        let mut unknown = task("task", Some("session-1"), false);
        unknown["kind"] = json!("future");
        assert!(parse_list_response(&json!({"result":{"tasks":[unknown]}}), "session-1").is_err());

        let oversized = vec![task("foreign", Some("session-2"), false); MAX_TASKS + 1];
        assert!(parse_list_response(&json!({"result":{"tasks":oversized}}), "session-1").is_err());
    }

    #[test]
    fn kill_requires_exact_envelope_id_and_known_outcome() {
        for (wire, expected) in [
            ("killed", BackgroundProcessStopOutcome::Killed),
            (
                "already_exited",
                BackgroundProcessStopOutcome::AlreadyExited,
            ),
            ("not_found", BackgroundProcessStopOutcome::NotFound),
        ] {
            assert_eq!(
                parse_kill_response(
                    &json!({"result":{"taskId":"task-1","outcome":wire}}),
                    "task-1"
                ),
                Ok(expected)
            );
        }
        assert!(parse_kill_response(
            &json!({"result":{"taskId":"task-2","outcome":"killed"}}),
            "task-1"
        )
        .is_err());
        assert!(parse_kill_response(
            &json!({"result":{"taskId":"task-1","outcome":"future"}}),
            "task-1"
        )
        .is_err());
        assert!(parse_kill_response(
            &json!({"result":null,"error":"session not found"}),
            "task-1"
        )
        .is_err());
    }
}
