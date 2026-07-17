use umadev_runtime::TurnInputBlock;

/// Drop only the editor-owned spacer immediately following a final chip.
/// Native-command whitespace and user-added whitespace remain byte-for-byte.
pub(super) fn submitted_content_end(raw: &str, final_marker_end: Option<usize>) -> usize {
    if raw.ends_with(' ') && final_marker_end.is_some_and(|end| end.saturating_add(1) == raw.len())
    {
        raw.len() - 1
    } else {
        raw.len()
    }
}

pub(super) fn append_text_block(blocks: &mut Vec<TurnInputBlock>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(TurnInputBlock::Text { text: existing }) = blocks.last_mut() {
        existing.push_str(text);
    } else {
        blocks.push(TurnInputBlock::Text {
            text: text.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::submitted_content_end;

    #[test]
    fn only_the_single_editor_separator_after_the_final_chip_is_removed() {
        assert_eq!(submitted_content_end("[Image 1] ", Some(9)), 9);
        assert_eq!(submitted_content_end("[Image 1]  ", Some(9)), 11);
        assert_eq!(submitted_content_end("command  ", None), 9);
    }
}
