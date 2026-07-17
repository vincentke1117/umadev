//! Grok Build's server-authoritative, versioned prompt-queue wire contract.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde_json::Value;

/// Queue snapshot notification emitted by Grok Build.
pub const QUEUE_CHANGED_METHOD: &str = "x.ai/queue/changed";
/// Remove one queued prompt if its version still matches.
pub const QUEUE_REMOVE_METHOD: &str = "x.ai/queue/remove";
/// Replace the visible ordering of queued prompt ids.
pub const QUEUE_REORDER_METHOD: &str = "x.ai/queue/reorder";
/// Remove all prompts owned by this client.
pub const QUEUE_CLEAR_METHOD: &str = "x.ai/queue/clear";
/// Edit one queued prompt in place.
pub const QUEUE_EDIT_METHOD: &str = "x.ai/queue/edit";
/// Atomically remove and interject one queued prompt.
pub const QUEUE_INTERJECT_METHOD: &str = "x.ai/queue/interject";

const MAX_QUEUE_ENTRIES: usize = 256;
const MAX_ID_BYTES: usize = 256;
const MAX_OWNER_BYTES: usize = 256;
const MAX_KIND_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// One row from the authoritative Grok queue snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokQueueEntry {
    id: String,
    version: u64,
    owner: Option<String>,
    last_editor: Option<String>,
    kind: String,
    text: String,
    position: usize,
}

impl GrokQueueEntry {
    /// Stable prompt id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Monotonic edit version.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Original enqueuing client, when present.
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// Most recent editing client, when present.
    #[must_use]
    pub fn last_editor(&self) -> Option<&str> {
        self.last_editor.as_deref()
    }

    /// Source display kind (`prompt`, `bash`, `command`, or `cron`).
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Plain queue text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Zero-based server position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }
}

/// A complete queue replacement for one exact live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokQueueSnapshot {
    session_id: String,
    entries: Vec<GrokQueueEntry>,
    running_prompt_id: Option<String>,
}

impl GrokQueueSnapshot {
    /// Parse and bind a `x.ai/queue/changed` payload to the active session.
    pub fn parse(params: &Value, active_session_id: &str) -> Result<Self, GrokQueueError> {
        validate_atom(active_session_id, MAX_ID_BYTES).map_err(|()| GrokQueueError::SessionId)?;
        let object = params.as_object().ok_or(GrokQueueError::Payload)?;
        let session_id = object
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(GrokQueueError::SessionId)?;
        validate_atom(session_id, MAX_ID_BYTES).map_err(|()| GrokQueueError::SessionId)?;
        if session_id != active_session_id {
            return Err(GrokQueueError::ForeignSession);
        }

        let rows = object
            .get("entries")
            .and_then(Value::as_array)
            .ok_or(GrokQueueError::Entries)?;
        if rows.len() > MAX_QUEUE_ENTRIES {
            return Err(GrokQueueError::TooManyEntries);
        }
        let mut ids = HashSet::with_capacity(rows.len());
        let mut entries = Vec::with_capacity(rows.len());
        for (expected_position, row) in rows.iter().enumerate() {
            let row = row.as_object().ok_or(GrokQueueError::Entry)?;
            let id = required_atom(row.get("id"), MAX_ID_BYTES, GrokQueueError::EntryId)?;
            if !ids.insert(id) {
                return Err(GrokQueueError::DuplicateEntryId);
            }
            let version = row.get("version").and_then(Value::as_u64).unwrap_or(0);
            let owner = optional_atom(row.get("owner"), MAX_OWNER_BYTES)?;
            let last_editor = optional_atom(row.get("lastEditor"), MAX_OWNER_BYTES)?;
            let kind = row.get("kind").and_then(Value::as_str).unwrap_or("");
            validate_optional_atom(kind, MAX_KIND_BYTES).map_err(|()| GrokQueueError::EntryKind)?;
            let text = row.get("text").and_then(Value::as_str).unwrap_or("");
            validate_prompt_text(text).map_err(|()| GrokQueueError::EntryText)?;
            let position = row
                .get("position")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(expected_position);
            if position != expected_position {
                return Err(GrokQueueError::EntryPosition);
            }
            entries.push(GrokQueueEntry {
                id: id.to_string(),
                version,
                owner: owner.map(str::to_string),
                last_editor: last_editor.map(str::to_string),
                kind: kind.to_string(),
                text: text.to_string(),
                position,
            });
        }

        let running_prompt_id = match object.get("runningPromptId") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => {
                validate_atom(id, MAX_ID_BYTES).map_err(|()| GrokQueueError::RunningPromptId)?;
                if ids.contains(id.as_str()) {
                    return Err(GrokQueueError::RunningPromptQueued);
                }
                Some(id.clone())
            }
            Some(_) => return Err(GrokQueueError::RunningPromptId),
        };

        Ok(Self {
            session_id: session_id.to_string(),
            entries,
            running_prompt_id,
        })
    }

    /// Session owning this replacement.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Authoritative ordered queue rows.
    #[must_use]
    pub fn entries(&self) -> &[GrokQueueEntry] {
        &self.entries
    }

    /// Prompt currently draining, when one is active.
    #[must_use]
    pub fn running_prompt_id(&self) -> Option<&str> {
        self.running_prompt_id.as_deref()
    }

    /// Convert the validated vendor wire shape into the runtime's portable
    /// server-authoritative queue event.
    #[must_use]
    pub fn into_runtime(self) -> umadev_runtime::PromptQueueSnapshot {
        umadev_runtime::PromptQueueSnapshot {
            session_id: self.session_id,
            entries: self
                .entries
                .into_iter()
                .map(|entry| umadev_runtime::PromptQueueEntry {
                    id: entry.id,
                    version: entry.version,
                    owner: entry.owner,
                    last_editor: entry.last_editor,
                    kind: entry.kind,
                    text: entry.text,
                    position: entry.position,
                })
                .collect(),
            running_prompt_id: self.running_prompt_id,
        }
    }
}

/// Local mirror of the latest accepted server snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrokPromptQueue {
    snapshot: Option<GrokQueueSnapshot>,
}

impl GrokPromptQueue {
    /// Apply a complete server replacement. A version regression is stale and
    /// leaves the current mirror unchanged.
    pub fn replace(&mut self, next: GrokQueueSnapshot) -> Result<(), GrokQueueError> {
        if let Some(current) = &self.snapshot {
            if current.session_id != next.session_id {
                return Err(GrokQueueError::ForeignSession);
            }
            for entry in &next.entries {
                if current
                    .entries
                    .iter()
                    .find(|known| known.id == entry.id)
                    .is_some_and(|known| entry.version < known.version)
                {
                    return Err(GrokQueueError::VersionRegression);
                }
            }
        }
        self.snapshot = Some(next);
        Ok(())
    }

    /// Latest authoritative snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Option<&GrokQueueSnapshot> {
        self.snapshot.as_ref()
    }
}

/// One fire-and-forget Grok queue mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrokQueueMutation {
    /// Version-checked remove.
    Remove {
        /// Stable queue id.
        id: String,
        /// Last server version the user acted on.
        expected_version: u64,
    },
    /// Reorder all currently visible ids.
    Reorder {
        /// Complete ordered id list.
        ordered_ids: Vec<String>,
    },
    /// Clear this client's queued prompts.
    Clear,
    /// Edit one queued prompt.
    Edit {
        /// Stable queue id.
        id: String,
        /// Replacement text.
        new_text: String,
    },
    /// Atomically edit (optionally), remove, and interject a queued prompt.
    Interject {
        /// Stable queue id.
        id: String,
        /// Last server version the user acted on.
        expected_version: u64,
        /// Optional non-empty replacement text.
        new_text: Option<String>,
    },
}

impl GrokQueueMutation {
    /// Exact extension-notification method and validated parameters.
    pub fn encode(&self, session_id: &str) -> Result<(&'static str, Value), GrokQueueError> {
        validate_atom(session_id, MAX_ID_BYTES).map_err(|()| GrokQueueError::SessionId)?;
        match self {
            Self::Remove {
                id,
                expected_version,
            } => {
                validate_atom(id, MAX_ID_BYTES).map_err(|()| GrokQueueError::EntryId)?;
                Ok((
                    QUEUE_REMOVE_METHOD,
                    serde_json::json!({
                        "sessionId": session_id,
                        "id": id,
                        "expectedVersion": expected_version,
                    }),
                ))
            }
            Self::Reorder { ordered_ids } => {
                if ordered_ids.len() > MAX_QUEUE_ENTRIES {
                    return Err(GrokQueueError::TooManyEntries);
                }
                let mut unique = HashSet::with_capacity(ordered_ids.len());
                for id in ordered_ids {
                    validate_atom(id, MAX_ID_BYTES).map_err(|()| GrokQueueError::EntryId)?;
                    if !unique.insert(id) {
                        return Err(GrokQueueError::DuplicateEntryId);
                    }
                }
                Ok((
                    QUEUE_REORDER_METHOD,
                    serde_json::json!({"sessionId": session_id, "orderedIds": ordered_ids}),
                ))
            }
            Self::Clear => Ok((
                QUEUE_CLEAR_METHOD,
                serde_json::json!({"sessionId": session_id}),
            )),
            Self::Edit { id, new_text } => {
                validate_atom(id, MAX_ID_BYTES).map_err(|()| GrokQueueError::EntryId)?;
                validate_nonempty_prompt_text(new_text)?;
                Ok((
                    QUEUE_EDIT_METHOD,
                    serde_json::json!({
                        "sessionId": session_id,
                        "id": id,
                        "newText": new_text,
                    }),
                ))
            }
            Self::Interject {
                id,
                expected_version,
                new_text,
            } => {
                validate_atom(id, MAX_ID_BYTES).map_err(|()| GrokQueueError::EntryId)?;
                let mut params = serde_json::json!({
                    "sessionId": session_id,
                    "id": id,
                    "expectedVersion": expected_version,
                });
                if let Some(text) = new_text {
                    validate_nonempty_prompt_text(text)?;
                    params["newText"] = Value::String(text.clone());
                }
                Ok((QUEUE_INTERJECT_METHOD, params))
            }
        }
    }
}

/// Build the exact Grok metadata for a normal queued prompt or Send Now.
pub fn grok_prompt_meta(prompt_id: &str, send_now: bool) -> Result<Value, GrokQueueError> {
    validate_atom(prompt_id, MAX_ID_BYTES).map_err(|()| GrokQueueError::EntryId)?;
    let mut meta = serde_json::json!({"promptId": prompt_id});
    if send_now {
        meta["sendNow"] = Value::Bool(true);
    }
    Ok(meta)
}

/// Queue contract violation. Messages never repeat untrusted content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokQueueError {
    /// Payload is not an object.
    Payload,
    /// Invalid or missing session id.
    SessionId,
    /// Snapshot belongs to another session.
    ForeignSession,
    /// Invalid or missing entries array.
    Entries,
    /// Queue exceeds the bounded UI/driver capacity.
    TooManyEntries,
    /// One row is not an object.
    Entry,
    /// Invalid row id.
    EntryId,
    /// Duplicate row id.
    DuplicateEntryId,
    /// Invalid row kind.
    EntryKind,
    /// Invalid or overlong row text.
    EntryText,
    /// Row positions are not the server's exact zero-based order.
    EntryPosition,
    /// Invalid running prompt id.
    RunningPromptId,
    /// The running prompt also appeared in the queued set.
    RunningPromptQueued,
    /// A same-id snapshot moved backwards in version.
    VersionRegression,
}

impl fmt::Display for GrokQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Payload => "invalid Grok queue payload",
            Self::SessionId => "invalid Grok queue session id",
            Self::ForeignSession => "Grok queue belongs to a foreign session",
            Self::Entries => "invalid Grok queue entries",
            Self::TooManyEntries => "Grok queue has too many entries",
            Self::Entry => "invalid Grok queue entry",
            Self::EntryId => "invalid Grok queue entry id",
            Self::DuplicateEntryId => "duplicate Grok queue entry id",
            Self::EntryKind => "invalid Grok queue entry kind",
            Self::EntryText => "invalid Grok queue entry text",
            Self::EntryPosition => "invalid Grok queue entry position",
            Self::RunningPromptId => "invalid Grok running prompt id",
            Self::RunningPromptQueued => "Grok running prompt is also queued",
            Self::VersionRegression => "stale Grok queue snapshot",
        };
        f.write_str(message)
    }
}

impl Error for GrokQueueError {}

fn required_atom(
    value: Option<&Value>,
    max_bytes: usize,
    error: GrokQueueError,
) -> Result<&str, GrokQueueError> {
    let value = value.and_then(Value::as_str).ok_or(error)?;
    validate_atom(value, max_bytes).map_err(|()| error)?;
    Ok(value)
}

fn optional_atom(value: Option<&Value>, max_bytes: usize) -> Result<Option<&str>, GrokQueueError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            validate_atom(value, max_bytes).map_err(|()| GrokQueueError::Entry)?;
            Ok(Some(value))
        }
        Some(_) => Err(GrokQueueError::Entry),
    }
}

fn validate_atom(value: &str, max_bytes: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Err(());
    }
    Ok(())
}

fn validate_optional_atom(value: &str, max_bytes: usize) -> Result<(), ()> {
    if value.len() > max_bytes
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Err(());
    }
    Ok(())
}

fn validate_prompt_text(value: &str) -> Result<(), ()> {
    if value.len() > MAX_TEXT_BYTES
        || value.chars().any(|ch| {
            ch == '\0'
                || matches!(ch, '\u{2028}' | '\u{2029}')
                || (ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        })
    {
        return Err(());
    }
    Ok(())
}

fn validate_nonempty_prompt_text(value: &str) -> Result<(), GrokQueueError> {
    validate_prompt_text(value).map_err(|()| GrokQueueError::EntryText)?;
    if value.trim().is_empty() {
        return Err(GrokQueueError::EntryText);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(version: u64) -> Value {
        serde_json::json!({
            "sessionId": "s1",
            "entries": [{
                "id": "p1",
                "version": version,
                "owner": "umadev",
                "lastEditor": "umadev",
                "kind": "prompt",
                "text": "fix the bug",
                "position": 0
            }],
            "runningPromptId": "p0"
        })
    }

    #[test]
    fn parses_the_pinned_source_wire_shape() {
        let parsed = GrokQueueSnapshot::parse(&snapshot(3), "s1").unwrap();
        assert_eq!(parsed.session_id(), "s1");
        assert_eq!(parsed.running_prompt_id(), Some("p0"));
        let row = &parsed.entries()[0];
        assert_eq!(row.id(), "p1");
        assert_eq!(row.version(), 3);
        assert_eq!(row.owner(), Some("umadev"));
        assert_eq!(row.last_editor(), Some("umadev"));
        assert_eq!(row.kind(), "prompt");
        assert_eq!(row.text(), "fix the bug");
        assert_eq!(row.position(), 0);
    }

    #[test]
    fn rejects_foreign_duplicate_and_malformed_snapshots() {
        assert_eq!(
            GrokQueueSnapshot::parse(&snapshot(0), "other"),
            Err(GrokQueueError::ForeignSession)
        );
        let mut duplicate = snapshot(0);
        duplicate["entries"] = serde_json::json!([
            {"id":"p1", "position":0},
            {"id":"p1", "position":1}
        ]);
        assert_eq!(
            GrokQueueSnapshot::parse(&duplicate, "s1"),
            Err(GrokQueueError::DuplicateEntryId)
        );
        let mut running_is_queued = snapshot(0);
        running_is_queued["runningPromptId"] = Value::String("p1".to_string());
        assert_eq!(
            GrokQueueSnapshot::parse(&running_is_queued, "s1"),
            Err(GrokQueueError::RunningPromptQueued)
        );
        let mut bad_position = snapshot(0);
        bad_position["entries"][0]["position"] = serde_json::json!(4);
        assert_eq!(
            GrokQueueSnapshot::parse(&bad_position, "s1"),
            Err(GrokQueueError::EntryPosition)
        );
    }

    #[test]
    fn mirror_is_server_authoritative_but_rejects_version_regression() {
        let mut mirror = GrokPromptQueue::default();
        mirror
            .replace(GrokQueueSnapshot::parse(&snapshot(3), "s1").unwrap())
            .unwrap();
        assert_eq!(
            mirror.replace(GrokQueueSnapshot::parse(&snapshot(2), "s1").unwrap()),
            Err(GrokQueueError::VersionRegression)
        );
        assert_eq!(mirror.snapshot().unwrap().entries()[0].version(), 3);

        let cleared = serde_json::json!({
            "sessionId": "s1",
            "entries": [],
            "runningPromptId": null
        });
        mirror
            .replace(GrokQueueSnapshot::parse(&cleared, "s1").unwrap())
            .unwrap();
        assert!(mirror.snapshot().unwrap().entries().is_empty());
    }

    #[test]
    fn mutations_match_the_source_notifications_exactly() {
        let cases = [
            (
                GrokQueueMutation::Remove {
                    id: "p1".into(),
                    expected_version: 4,
                },
                QUEUE_REMOVE_METHOD,
                serde_json::json!({"sessionId":"s1","id":"p1","expectedVersion":4}),
            ),
            (
                GrokQueueMutation::Reorder {
                    ordered_ids: vec!["p2".into(), "p1".into()],
                },
                QUEUE_REORDER_METHOD,
                serde_json::json!({"sessionId":"s1","orderedIds":["p2","p1"]}),
            ),
            (
                GrokQueueMutation::Clear,
                QUEUE_CLEAR_METHOD,
                serde_json::json!({"sessionId":"s1"}),
            ),
            (
                GrokQueueMutation::Edit {
                    id: "p1".into(),
                    new_text: "new".into(),
                },
                QUEUE_EDIT_METHOD,
                serde_json::json!({"sessionId":"s1","id":"p1","newText":"new"}),
            ),
            (
                GrokQueueMutation::Interject {
                    id: "p1".into(),
                    expected_version: 4,
                    new_text: Some("now".into()),
                },
                QUEUE_INTERJECT_METHOD,
                serde_json::json!({
                    "sessionId":"s1","id":"p1","expectedVersion":4,"newText":"now"
                }),
            ),
        ];
        for (mutation, expected_method, expected_params) in cases {
            let (method, params) = mutation.encode("s1").unwrap();
            assert_eq!(method, expected_method);
            assert_eq!(params, expected_params);
        }
    }

    #[test]
    fn stale_sensitive_mutations_carry_the_seen_version() {
        let (_, remove) = GrokQueueMutation::Remove {
            id: "p9".into(),
            expected_version: 17,
        }
        .encode("s1")
        .unwrap();
        let (_, interject) = GrokQueueMutation::Interject {
            id: "p9".into(),
            expected_version: 17,
            new_text: None,
        }
        .encode("s1")
        .unwrap();
        assert_eq!(remove["expectedVersion"], 17);
        assert_eq!(interject["expectedVersion"], 17);
        assert!(interject.get("newText").is_none());
    }

    #[test]
    fn prompt_metadata_distinguishes_queue_from_send_now() {
        assert_eq!(
            grok_prompt_meta("p1", false).unwrap(),
            serde_json::json!({"promptId":"p1"})
        );
        assert_eq!(
            grok_prompt_meta("p1", true).unwrap(),
            serde_json::json!({"promptId":"p1","sendNow":true})
        );
    }
}
