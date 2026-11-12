use std::sync::Arc;

use anyhow::{Context as _, Result};
use extension::ghost_text::{GhostTextHost, GhostTextSession, validate_offset};
use extension::guided_typing::{GuidedTyping, Progress};
use gpui::{App, AppContext as _, Context, Entity, HighlightStyle, WeakEntity};
use language::Buffer;
use multi_buffer::MultiBufferOffset;
use project::InlayId;
use theme::ActiveTheme as _;
use workspace::MultiWorkspace;

use crate::{Autoscroll, Editor, HighlightKey, Inlay, inlays::InlayContent};

const GHOST_TEXT_ID: InlayId = InlayId::ExtensionGhostText(0);

fn scratch_buffer(editor: &Editor, cx: &App) -> Result<Entity<Buffer>> {
    let buffer = editor
        .buffer
        .read(cx)
        .as_singleton()
        .context("ghost text requires a scratch buffer")?;
    anyhow::ensure!(
        buffer.read(cx).file().is_none(),
        "ghost text editor is no longer a scratch buffer"
    );
    Ok(buffer)
}

pub fn init(cx: &mut App) {
    cx.set_global(GhostTextHost(open));
}

fn open(title: String, text: String, cx: &mut App) -> Result<Arc<dyn GhostTextSession>> {
    let window = cx
        .active_window()
        .context("open a workspace before creating ghost text")?
        .downcast::<MultiWorkspace>()
        .context("active window is not a workspace")?;
    window.update(cx, |multi_workspace, window, cx| {
        let workspace = multi_workspace.workspace().clone();
        workspace.update(cx, |workspace, cx| {
            let buffer = cx.new(|cx| Buffer::local(text, cx));
            let editor = cx.new(|cx| {
                let mut editor = Editor::for_buffer(buffer, None, window, cx);
                editor.autoindent_mode = None;
                editor.set_use_autoclose(false);
                editor.set_show_edit_predictions(Some(false), window, cx);
                editor
                    .buffer
                    .update(cx, |buffer, cx| buffer.set_title(title, cx));
                editor
            });
            workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
            Arc::new(Session::new(editor.downgrade(), cx)) as Arc<dyn GhostTextSession>
        })
    })
}

struct Session {
    editor: WeakEntity<Editor>,
    _lifetime: Entity<SessionLifetime>,
}

struct SessionLifetime {
    editor: WeakEntity<Editor>,
}

impl Session {
    fn new(editor: WeakEntity<Editor>, cx: &mut App) -> Self {
        let _lifetime = cx.new(|cx| {
            cx.on_release(|session: &mut SessionLifetime, cx| {
                if let Err(error) = session.editor.update(cx, |editor, cx| {
                    editor.cancel_guided_typing(cx);
                    editor.splice_inlays(&[GHOST_TEXT_ID], Vec::new(), cx);
                }) {
                    log::debug!("releasing closed ghost text session: {error}");
                }
            })
            .detach();
            SessionLifetime {
                editor: editor.clone(),
            }
        });
        Self { editor, _lifetime }
    }
}

pub(super) struct GuidedSession {
    model: GuidedTyping,
    progress: Progress,
    title: String,
}

impl Editor {
    pub(super) fn refresh_guided_typing(&mut self, cx: &mut Context<Self>) {
        if self.guided_typing.is_none() {
            return;
        }
        if scratch_buffer(self, cx).is_err() {
            self.cancel_guided_typing(cx);
            self.splice_inlays(&[GHOST_TEXT_ID], Vec::new(), cx);
            return;
        }
        let text = self.text(cx);
        let Some(mut session) = self.guided_typing.take() else {
            return;
        };
        let previous_change = session.progress.completed_changes;
        session.progress = session.model.update(&text);
        let progress = &session.progress;
        let status = if progress.complete {
            "Complete".to_string()
        } else if progress.invalid_context {
            "Restore unchanged context to continue".to_string()
        } else if progress.delete_only {
            format!(
                "Change {}/{} · Delete {} characters",
                progress.completed_changes + 1,
                progress.total_changes,
                progress.remove_characters
            )
        } else if progress.remove_characters > 0 {
            format!(
                "Change {}/{} · {} incorrect characters",
                progress.completed_changes + 1,
                progress.total_changes,
                progress.remove_characters
            )
        } else if progress.ghost_text.starts_with('\n') {
            format!(
                "Change {}/{} · press Enter",
                progress.completed_changes + 1,
                progress.total_changes
            )
        } else {
            format!(
                "Change {}/{} · type ghost text",
                progress.completed_changes + 1,
                progress.total_changes
            )
        };
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_title(format!("{} — {}", session.title, status), cx)
        });
        let inlays = if progress.ghost_text.is_empty() {
            Vec::new()
        } else {
            let position = self
                .buffer
                .read(cx)
                .snapshot(cx)
                .anchor_after(MultiBufferOffset(progress.ghost_offset));
            vec![Inlay {
                id: GHOST_TEXT_ID,
                position,
                content: InlayContent::Text(progress.ghost_text.clone().into()),
            }]
        };
        self.splice_inlays(&[GHOST_TEXT_ID], inlays, cx);
        self.clear_highlights(HighlightKey::GuidedTypingError, cx);
        if !progress.error_ranges.is_empty() {
            let buffer = self.buffer.read(cx).snapshot(cx);
            let ranges = progress
                .error_ranges
                .iter()
                .map(|range| {
                    buffer.anchor_after(MultiBufferOffset(range.start))
                        ..buffer.anchor_before(MultiBufferOffset(range.end))
                })
                .collect();
            self.highlight_text(
                HighlightKey::GuidedTypingError,
                ranges,
                HighlightStyle {
                    color: Some(cx.theme().status().error),
                    background_color: Some(cx.theme().status().error_background),
                    ..HighlightStyle::default()
                },
                cx,
            );
        }
        if progress.completed_changes != previous_change
            && !progress.complete
            && !progress.invalid_context
        {
            let snapshot = self.display_snapshot(cx);
            self.selections.change_with(&snapshot, |selections| {
                selections.select_ranges([MultiBufferOffset(progress.cursor_offset)
                    ..MultiBufferOffset(progress.cursor_offset)])
            });
            self.request_autoscroll(Autoscroll::fit(), cx);
        }
        self.guided_typing = Some(session);
        cx.notify();
    }

    fn cancel_guided_typing(&mut self, cx: &mut Context<Self>) {
        self.clear_highlights(HighlightKey::GuidedTypingError, cx);
        if let Some(session) = self.guided_typing.take() {
            self.buffer
                .update(cx, |buffer, cx| buffer.set_title(session.title, cx));
        }
    }
}

impl GhostTextSession for Session {
    fn start_guided_typing(&self, mut target: String, cx: &mut App) -> Result<Progress> {
        text::LineEnding::normalize(&mut target);
        self.editor.update(cx, |editor, cx| {
            let buffer = scratch_buffer(editor, cx)?;
            editor.cancel_guided_typing(cx);
            let before = editor.text(cx);
            let title = editor.buffer.read(cx).title(cx).to_string();
            let model = GuidedTyping::new(before, target);
            if editor.text(cx) != model.initial_text() {
                buffer.update(cx, |buffer, cx| buffer.set_text(model.initial_text(), cx));
            }
            // Keep prompt preparation out of the user's first typing undo group.
            editor.finalize_last_transaction(cx);
            editor.guided_typing = Some(GuidedSession {
                model,
                progress: Progress::default(),
                title,
            });
            editor.refresh_guided_typing(cx);
            let progress = editor
                .guided_typing
                .as_ref()
                .context("guided typing session missing")?
                .progress
                .clone();
            if !progress.complete {
                let snapshot = editor.display_snapshot(cx);
                editor.selections.change_with(&snapshot, |selections| {
                    selections.select_ranges([MultiBufferOffset(progress.cursor_offset)
                        ..MultiBufferOffset(progress.cursor_offset)])
                });
                editor.request_autoscroll(Autoscroll::fit(), cx);
            }
            Ok(progress)
        })?
    }

    fn progress(&self, cx: &App) -> Result<Progress> {
        let editor = self
            .editor
            .upgrade()
            .context("ghost text editor was closed")?;
        Ok(editor
            .read(cx)
            .guided_typing
            .as_ref()
            .context("no guided typing session is active")?
            .progress
            .clone())
    }

    fn set(&self, offset: usize, text: String, cx: &mut App) -> Result<()> {
        self.editor.update(cx, |editor, cx| {
            scratch_buffer(editor, cx)?;
            validate_offset(&editor.text(cx), offset)?;
            editor.cancel_guided_typing(cx);
            let position = editor
                .buffer
                .read(cx)
                .snapshot(cx)
                .anchor_after(MultiBufferOffset(offset));
            let inlays = if text.is_empty() {
                Vec::new()
            } else {
                vec![Inlay {
                    id: GHOST_TEXT_ID,
                    position,
                    content: InlayContent::Text(text.into()),
                }]
            };
            editor.splice_inlays(&[GHOST_TEXT_ID], inlays, cx);
            Ok(())
        })?
    }

    fn text(&self, cx: &App) -> Result<String> {
        let editor = self
            .editor
            .upgrade()
            .context("ghost text editor was closed")?;
        scratch_buffer(editor.read(cx), cx)?;
        Ok(editor.read(cx).text(cx))
    }

    fn clear(&self, cx: &mut App) -> Result<()> {
        self.editor.update(cx, |editor, cx| {
            editor.cancel_guided_typing(cx);
            editor.splice_inlays(&[GHOST_TEXT_ID], Vec::new(), cx)
        })
    }
}
