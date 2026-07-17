//! Grok Build's server-authoritative prompt-queue view model.
//!
//! This module deliberately owns no local queue. A key press can produce a
//! versioned mutation, but the visible rows change only when the base sends a
//! complete [`PromptQueueSnapshot`]. That separation prevents a transport write
//! (or a stale mutation) from looking like a committed delete/reorder.

use umadev_runtime::{PromptQueueEntry, PromptQueueMutation, PromptQueueSnapshot};

/// Maximum number of queue entries visible at once.
pub(crate) const MAX_VISIBLE_ROWS: usize = 3;

/// Pure state for the queue pane and queued-row editor.
#[derive(Debug, Clone, Default)]
pub(crate) struct PromptQueueUi {
    ready: bool,
    open: bool,
    snapshot: Option<PromptQueueSnapshot>,
    selected_id: Option<String>,
    editing_id: Option<String>,
    awaiting_snapshot: bool,
}

impl PromptQueueUi {
    /// Publish whether the current live session negotiated the native queue.
    pub(crate) fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
        if !ready {
            self.open = false;
            self.snapshot = None;
            self.selected_id = None;
            self.editing_id = None;
            self.awaiting_snapshot = false;
        }
    }

    pub(crate) fn ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn awaiting_snapshot(&self) -> bool {
        self.awaiting_snapshot
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing_id.is_some()
    }

    /// Toggle the native pane. Unsupported sessions consume no shortcut.
    pub(crate) fn toggle(&mut self) -> bool {
        if !self.ready {
            return false;
        }
        self.open = !self.open;
        true
    }

    /// Replace the complete mirror from one authoritative base snapshot.
    pub(crate) fn apply_snapshot(&mut self, snapshot: PromptQueueSnapshot) {
        let mutation_was_pending = self.awaiting_snapshot;
        let edited_row_still_exists = self
            .editing_id
            .as_ref()
            .is_some_and(|edited| snapshot.entries.iter().any(|entry| &entry.id == edited));
        let selected_still_exists = self
            .selected_id
            .as_ref()
            .is_some_and(|selected| snapshot.entries.iter().any(|entry| &entry.id == selected));
        if !selected_still_exists {
            self.selected_id = snapshot.entries.first().map(|entry| entry.id.clone());
        }
        self.snapshot = Some(snapshot);
        // A later snapshot is the only acknowledgement for every mutation. It
        // may contain the old value (stale/rejected); either way it is truth.
        self.awaiting_snapshot = false;
        // An unsolicited snapshot can arrive while the user is still drafting
        // an edit. Keep that editor attached when its row survives; only a
        // post-mutation snapshot or row deletion completes/cancels the edit.
        if mutation_was_pending || !edited_row_still_exists {
            self.editing_id = None;
        }
    }

    pub(crate) fn reject_pending(&mut self) {
        self.awaiting_snapshot = false;
    }

    pub(crate) fn entries(&self) -> &[PromptQueueEntry] {
        self.snapshot
            .as_ref()
            .map_or(&[], |snapshot| snapshot.entries.as_slice())
    }

    pub(crate) fn selected_id(&self) -> Option<&str> {
        self.selected_id.as_deref()
    }

    pub(crate) fn selected_entry(&self) -> Option<&PromptQueueEntry> {
        let selected = self.selected_id.as_deref()?;
        self.entries().iter().find(|entry| entry.id == selected)
    }

    pub(crate) fn select_next(&mut self) {
        self.move_selection(1);
    }

    pub(crate) fn select_previous(&mut self) {
        self.move_selection(-1);
    }

    fn move_selection(&mut self, delta: isize) {
        let entries = self.entries();
        if entries.is_empty() {
            self.selected_id = None;
            return;
        }
        let current = self
            .selected_id
            .as_deref()
            .and_then(|id| entries.iter().position(|entry| entry.id == id))
            .unwrap_or(0);
        let len = entries.len();
        let next = if delta < 0 {
            current.checked_sub(1).unwrap_or(len - 1)
        } else {
            (current + 1) % len
        };
        self.selected_id = Some(entries[next].id.clone());
    }

    /// Start editing without changing the server mirror.
    pub(crate) fn begin_edit(&mut self) -> Option<String> {
        if self.awaiting_snapshot {
            return None;
        }
        let entry = self.selected_entry()?;
        let id = entry.id.clone();
        let text = entry.text.clone();
        self.editing_id = Some(id);
        self.open = false;
        Some(text)
    }

    /// Build an edit request; the displayed row remains unchanged until the
    /// subsequent snapshot.
    pub(crate) fn submit_edit(&mut self, new_text: String) -> Option<PromptQueueMutation> {
        let id = self.editing_id.clone()?;
        if new_text.trim().is_empty() || self.awaiting_snapshot {
            return None;
        }
        self.awaiting_snapshot = true;
        Some(PromptQueueMutation::Edit { id, new_text })
    }

    pub(crate) fn remove_selected(&mut self) -> Option<PromptQueueMutation> {
        if self.awaiting_snapshot {
            return None;
        }
        let entry = self.selected_entry()?;
        let mutation = PromptQueueMutation::Remove {
            id: entry.id.clone(),
            expected_version: entry.version,
        };
        self.awaiting_snapshot = true;
        Some(mutation)
    }

    pub(crate) fn interject_selected(&mut self) -> Option<PromptQueueMutation> {
        if self.awaiting_snapshot {
            return None;
        }
        let entry = self.selected_entry()?;
        let mutation = PromptQueueMutation::Interject {
            id: entry.id.clone(),
            expected_version: entry.version,
            new_text: None,
        };
        self.awaiting_snapshot = true;
        Some(mutation)
    }

    /// Empty-composer Enter always promotes the first server row, independent
    /// of pane selection, matching Grok Build's prompt-path behavior.
    pub(crate) fn interject_top(&mut self) -> Option<PromptQueueMutation> {
        if self.awaiting_snapshot {
            return None;
        }
        let entry = self.entries().first()?;
        let mutation = PromptQueueMutation::Interject {
            id: entry.id.clone(),
            expected_version: entry.version,
            new_text: None,
        };
        self.awaiting_snapshot = true;
        Some(mutation)
    }

    /// Return a full ordered-id mutation while leaving the visible order alone.
    pub(crate) fn reorder_selected(&mut self, upward: bool) -> Option<PromptQueueMutation> {
        if self.awaiting_snapshot {
            return None;
        }
        let selected = self.selected_id.as_deref()?;
        let mut ids: Vec<String> = self
            .entries()
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        let position = ids.iter().position(|id| id == selected)?;
        let other = if upward {
            position.checked_sub(1)?
        } else {
            let next = position + 1;
            (next < ids.len()).then_some(next)?
        };
        ids.swap(position, other);
        self.awaiting_snapshot = true;
        Some(PromptQueueMutation::Reorder { ordered_ids: ids })
    }

    /// Selected-centered, bounded server rows for rendering.
    pub(crate) fn visible_entries(&self) -> &[PromptQueueEntry] {
        let entries = self.entries();
        if entries.len() <= MAX_VISIBLE_ROWS {
            return entries;
        }
        let selected = self
            .selected_id
            .as_deref()
            .and_then(|id| entries.iter().position(|entry| entry.id == id))
            .unwrap_or(0);
        let start = selected
            .saturating_sub(MAX_VISIBLE_ROWS / 2)
            .min(entries.len() - MAX_VISIBLE_ROWS);
        &entries[start..start + MAX_VISIBLE_ROWS]
    }

    pub(crate) fn panel_height(&self) -> u16 {
        if !self.open {
            return 0;
        }
        let rows = self.entries().len().min(MAX_VISIBLE_ROWS);
        u16::try_from(rows.saturating_add(2)).unwrap_or(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(version: u64, texts: &[&str]) -> PromptQueueSnapshot {
        PromptQueueSnapshot {
            session_id: "s1".to_string(),
            entries: texts
                .iter()
                .enumerate()
                .map(|(position, text)| PromptQueueEntry {
                    id: format!("p{position}"),
                    version,
                    owner: Some("umadev".to_string()),
                    last_editor: None,
                    kind: "prompt".to_string(),
                    text: (*text).to_string(),
                    position,
                })
                .collect(),
            running_prompt_id: Some("running".to_string()),
        }
    }

    #[test]
    fn remove_and_reorder_never_change_rows_before_snapshot() {
        let mut ui = PromptQueueUi::default();
        ui.set_ready(true);
        ui.apply_snapshot(snapshot(7, &["one", "two", "three", "four"]));
        assert!(ui.toggle());
        let before: Vec<_> = ui
            .entries()
            .iter()
            .map(|entry| entry.text.clone())
            .collect();
        assert_eq!(
            ui.remove_selected(),
            Some(PromptQueueMutation::Remove {
                id: "p0".to_string(),
                expected_version: 7,
            })
        );
        assert_eq!(
            ui.entries()
                .iter()
                .map(|entry| entry.text.clone())
                .collect::<Vec<_>>(),
            before
        );
        assert!(
            ui.remove_selected().is_none(),
            "one mutation awaits one snapshot"
        );
        ui.apply_snapshot(snapshot(8, &["one", "two", "three", "four"]));
        assert_eq!(
            ui.reorder_selected(false),
            Some(PromptQueueMutation::Reorder {
                ordered_ids: vec![
                    "p1".to_string(),
                    "p0".to_string(),
                    "p2".to_string(),
                    "p3".to_string()
                ],
            })
        );
        assert_eq!(ui.entries()[0].text, "one", "no optimistic reorder");
    }

    #[test]
    fn stale_snapshot_replaces_everything_and_preserves_a_valid_selection() {
        let mut ui = PromptQueueUi::default();
        ui.set_ready(true);
        ui.apply_snapshot(snapshot(2, &["one", "two"]));
        ui.select_next();
        assert_eq!(ui.selected_id(), Some("p1"));
        let _ = ui.remove_selected();
        ui.apply_snapshot(snapshot(3, &["server kept one", "server kept two"]));
        assert_eq!(ui.selected_id(), Some("p1"));
        assert_eq!(
            ui.selected_entry().map(|entry| entry.text.as_str()),
            Some("server kept two")
        );
        assert!(!ui.awaiting_snapshot());
    }

    #[test]
    fn visible_rows_are_capped_and_empty_enter_uses_top_not_selection() {
        let mut ui = PromptQueueUi::default();
        ui.set_ready(true);
        ui.apply_snapshot(snapshot(4, &["one", "two", "three", "four", "five"]));
        ui.select_next();
        ui.select_next();
        ui.select_next();
        assert_eq!(ui.visible_entries().len(), MAX_VISIBLE_ROWS);
        assert_eq!(
            ui.interject_top(),
            Some(PromptQueueMutation::Interject {
                id: "p0".to_string(),
                expected_version: 4,
                new_text: None,
            })
        );
    }

    #[test]
    fn unsolicited_snapshot_does_not_turn_an_in_progress_edit_into_a_new_prompt() {
        let mut ui = PromptQueueUi::default();
        ui.set_ready(true);
        ui.apply_snapshot(snapshot(4, &["one", "two"]));
        assert_eq!(ui.begin_edit().as_deref(), Some("one"));

        ui.apply_snapshot(snapshot(5, &["server refreshed one", "two"]));
        assert!(ui.is_editing());
        assert_eq!(
            ui.submit_edit("my edited one".to_string()),
            Some(PromptQueueMutation::Edit {
                id: "p0".to_string(),
                new_text: "my edited one".to_string(),
            })
        );

        ui.apply_snapshot(snapshot(6, &["my edited one", "two"]));
        assert!(!ui.is_editing());
    }
}
