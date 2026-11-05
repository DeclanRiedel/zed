use std::sync::Arc;

use anyhow::{Context as _, Result};
use extension::ghost_text::{GhostTextHost, GhostTextSession, validate_offset};
use gpui::{App, AppContext as _, Entity, WeakEntity};
use language::Buffer;
use multi_buffer::MultiBufferOffset;
use project::InlayId;
use workspace::MultiWorkspace;

use crate::{Editor, Inlay, inlays::InlayContent};

const GHOST_TEXT_ID: InlayId = InlayId::ExtensionGhostText(0);

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

impl GhostTextSession for Session {
    fn set(&self, offset: usize, text: String, cx: &mut App) -> Result<()> {
        self.editor.update(cx, |editor, cx| {
            validate_offset(&editor.text(cx), offset)?;
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
        Ok(self
            .editor
            .upgrade()
            .context("ghost text editor was closed")?
            .read(cx)
            .text(cx))
    }

    fn clear(&self, cx: &mut App) -> Result<()> {
        self.editor.update(cx, |editor, cx| {
            editor.splice_inlays(&[GHOST_TEXT_ID], Vec::new(), cx)
        })
    }
}
