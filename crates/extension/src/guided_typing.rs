//! Buffer-independent guided typing state, shared by the extension host and editor.

use std::ops::Range;

use similar::{ChangeTag, TextDiff};

#[derive(Clone, Debug)]
struct Edit {
    range: Range<usize>,
    text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub completed_changes: u32,
    pub total_changes: u32,
    pub complete: bool,
    pub invalid_context: bool,
    pub remove_characters: u32,
    pub ghost_offset: usize,
    pub cursor_offset: usize,
    pub ghost_text: String,
    pub error_ranges: Vec<Range<usize>>,
    pub delete_only: bool,
}

#[derive(Clone, Debug)]
pub struct GuidedTyping {
    initial_text: String,
    baseline: String,
    target: String,
    edits: Vec<Edit>,
    completed: usize,
    shift: isize,
}

impl GuidedTyping {
    pub fn new(before: String, target: String) -> Self {
        let mut changes = Vec::new();
        let mut offset = 0;
        let mut pending: Option<(usize, String, String)> = None;
        for change in TextDiff::configure()
            .timeout(std::time::Duration::from_millis(100))
            .diff_lines(&before, &target)
            .iter_all_changes()
        {
            match change.tag() {
                ChangeTag::Equal => {
                    if let Some((start, old, new)) = pending.take() {
                        changes.push(Edit {
                            range: start..start + old.len(),
                            text: new,
                        });
                    }
                    offset += change.value().len();
                }
                ChangeTag::Delete => {
                    let entry =
                        pending.get_or_insert_with(|| (offset, String::new(), String::new()));
                    entry.1.push_str(change.value());
                    offset += change.value().len();
                }
                ChangeTag::Insert => {
                    let entry =
                        pending.get_or_insert_with(|| (offset, String::new(), String::new()));
                    entry.2.push_str(change.value());
                }
            }
        }
        if let Some((start, old, new)) = pending {
            changes.push(Edit {
                range: start..start + old.len(),
                text: new,
            });
        }

        let mut initial_text = String::new();
        let mut previous_end = 0;
        let mut edits = Vec::new();
        for change in changes {
            initial_text.push_str(&before[previous_end..change.range.start]);
            let start = initial_text.len();
            // Changed rows with a replacement are typed from column one. Keep
            // deletion-only rows so Delete still exercises their removal.
            if change.text.is_empty() {
                initial_text.push_str(&before[change.range.clone()]);
            }
            edits.push(Edit {
                range: start..initial_text.len(),
                text: change.text,
            });
            previous_end = change.range.end;
        }
        initial_text.push_str(&before[previous_end..]);
        Self {
            baseline: initial_text.clone(),
            initial_text,
            target,
            edits,
            completed: 0,
            shift: 0,
        }
    }

    pub fn initial_text(&self) -> &str {
        &self.initial_text
    }

    pub fn update(&mut self, current: &str) -> Progress {
        let mut progress = Progress {
            completed_changes: self.completed as u32,
            total_changes: self.edits.len() as u32,
            ..Progress::default()
        };
        while let Some(edit) = self.edits.get(self.completed) {
            let start = edit.range.start.saturating_add_signed(self.shift);
            let end = edit.range.end.saturating_add_signed(self.shift);
            let prefix = &self.baseline[..start];
            let suffix = &self.baseline[end..];
            if current.len() < prefix.len() + suffix.len()
                || !current.starts_with(prefix)
                || !current.ends_with(suffix)
            {
                progress.invalid_context = true;
                return progress;
            }
            let middle = &current[start..current.len() - suffix.len()];
            if middle == edit.text {
                self.baseline.replace_range(start..end, &edit.text);
                self.shift += edit.text.len() as isize - edit.range.len() as isize;
                self.completed += 1;
                progress.completed_changes = self.completed as u32;
                continue;
            }

            let mut expected = edit.text.chars();
            for (offset, character) in middle.char_indices() {
                if expected.next() != Some(character) {
                    progress.remove_characters += 1;
                    let range = start + offset..start + offset + character.len_utf8();
                    match progress.error_ranges.last_mut() {
                        Some(previous) if previous.end == range.start => previous.end = range.end,
                        _ => progress.error_ranges.push(range),
                    }
                }
            }
            progress.ghost_offset = start + middle.len();
            progress.delete_only = edit.text.is_empty();
            progress.cursor_offset = if progress.delete_only {
                start
            } else {
                progress.ghost_offset
            };
            // A wrong character occupies its expected slot, just like a correct
            // character, so it is shaded rather than pushing the prompt aside.
            progress.ghost_text = expected.collect();
            return progress;
        }
        progress.complete = current == self.target;
        progress.invalid_context = !progress.complete;
        progress.ghost_offset = current.len();
        progress.cursor_offset = current.len();
        progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_rows_start_empty_and_wrong_characters_consume_slots() {
        let mut session = GuidedTyping::new("old\n".into(), "hello\n".into());
        assert_eq!(session.initial_text(), "");
        assert_eq!(session.update("").ghost_text, "hello\n");
        let progress = session.update("heX");
        assert_eq!(progress.ghost_text, "lo\n");
        assert_eq!(progress.remove_characters, 1);
        assert_eq!(progress.error_ranges, vec![2..3]);
        assert_eq!(progress.cursor_offset, 3);
        assert_eq!(session.update("he").ghost_text, "llo\n");
        assert!(session.update("hello\n").complete);
    }

    #[test]
    fn unicode_errors_are_highlighted_on_character_boundaries() {
        let mut session = GuidedTyping::new("let café = 1;\n".into(), "let café = 你好;\n".into());
        assert_eq!(session.initial_text(), "");
        let prefix = "let café = ";
        let progress = session.update(&format!("{prefix}好"));
        assert_eq!(progress.ghost_text, "好;\n");
        assert_eq!(progress.error_ranges, vec![prefix.len()..prefix.len() + 3]);
        assert_eq!(progress.remove_characters, 1);
        assert_eq!(session.update(prefix).ghost_text, "你好;\n");
        assert!(session.update("let café = 你好;\n").complete);
    }

    #[test]
    fn deletion_and_multiple_hunks_preserve_unchanged_lines() {
        let mut session = GuidedTyping::new("remove\nkeep\nold\n".into(), "keep\nnew\n".into());
        assert_eq!(session.initial_text(), "remove\nkeep\n");
        let progress = session.update("remove\nkeep\n");
        assert_eq!(progress.total_changes, 2);
        assert!(progress.delete_only);
        assert_eq!(progress.cursor_offset, 0);
        assert_eq!(progress.remove_characters, 7);
        let progress = session.update("keep\n");
        assert_eq!(progress.completed_changes, 1);
        assert_eq!(progress.ghost_text, "new\n");
        assert_eq!(progress.cursor_offset, 5);
        assert!(session.update("keep\nnew\n").complete);
    }

    #[test]
    fn context_edits_pause_and_undo_resumes() {
        let mut session = GuidedTyping::new("keep\nold\n".into(), "keep\nnew\n".into());
        assert_eq!(session.initial_text(), "keep\n");
        assert!(session.update("oops\n").invalid_context);
        assert!(!session.update("keep\nn").invalid_context);
        assert!(session.update("keep\nnew\n").complete);
        assert!(session.update("keep\nold\n").invalid_context);
        assert!(session.update("keep\nnew\n").complete);
    }

    #[test]
    fn empty_and_no_final_newline() {
        assert!(
            GuidedTyping::new(String::new(), String::new())
                .update("")
                .complete
        );
        let mut session = GuidedTyping::new(String::new(), "é".into());
        assert_eq!(session.update("").ghost_text, "é");
        let progress = session.update("你");
        assert!(progress.ghost_text.is_empty());
        assert_eq!(progress.error_ranges, vec![0..3]);
        assert!(session.update("é").complete);
        assert!(
            GuidedTyping::new("old".into(), String::new())
                .update("")
                .complete
        );
    }

    fn retype(before: &str, target: &str) {
        let mut session = GuidedTyping::new(before.into(), target.into());
        let mut current = session.initial_text().to_owned();
        for _ in 0..1000 {
            let progress = session.update(&current);
            assert!(!progress.invalid_context, "{current:?} -> {target:?}");
            if progress.complete {
                assert_eq!(current, target);
                return;
            }
            if progress.remove_characters > 0 {
                assert!(progress.delete_only);
                let start = progress.cursor_offset;
                let size = current[start..].chars().next().unwrap().len_utf8();
                current.replace_range(start..start + size, "");
            } else {
                let next = progress
                    .ghost_text
                    .chars()
                    .next()
                    .expect("unfinished exercise must guide an edit");
                current.insert(progress.ghost_offset, next);
            }
        }
        panic!("exercise did not finish: {before:?} -> {target:?}");
    }
    #[test]
    fn varied_text_changes_converge_with_character_input() {
        let samples = [
            "",
            "a",
            "é",
            "你好",
            "a\nb\nc\n",
            "a\nkeep\nb\n",
            "z\nkeep\ny\n",
            "\n\n",
            "tabs\tand spaces\r\n",
        ];
        for before in samples {
            for after in samples {
                retype(before, after);
            }
        }
    }
}
