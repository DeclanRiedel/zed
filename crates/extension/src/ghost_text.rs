use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{App, Global};

/// A scratch editor owned by an extension. Offsets are UTF-8 byte offsets.
pub trait GhostTextSession: Send + Sync {
    fn set(&self, offset: usize, text: String, cx: &mut App) -> Result<()>;
    fn text(&self, cx: &App) -> Result<String>;
    fn clear(&self, cx: &mut App) -> Result<()>;
}

pub struct GhostTextHost(pub fn(String, String, &mut App) -> Result<Arc<dyn GhostTextSession>>);
impl Global for GhostTextHost {}

pub fn open(title: String, text: String, cx: &mut App) -> Result<Arc<dyn GhostTextSession>> {
    let open = cx
        .try_global::<GhostTextHost>()
        .context("ghost text editors are unavailable")?
        .0;
    open(title, text, cx)
}

pub fn validate_offset(text: &str, offset: usize) -> Result<()> {
    anyhow::ensure!(
        text.is_char_boundary(offset),
        "ghost text offset must be a UTF-8 character boundary within the buffer"
    );
    Ok(())
}
