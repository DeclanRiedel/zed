use anyhow::{Context as _, Result};
use gpui::{App, Global, Task};

#[derive(Clone, Copy, Debug)]
pub enum GitDiffKind {
    Staged,
    Unstaged,
}

/// Immutable snapshots. Missing file sides are represented by empty text.
#[derive(Debug)]
pub struct GitFileDiff {
    pub before_text: String,
    pub after_text: String,
}

pub struct GitDiffHost(pub fn(u64, String, GitDiffKind, &mut App) -> Task<Result<GitFileDiff>>);
impl Global for GitDiffHost {}

pub fn read(
    worktree_id: u64,
    path: String,
    kind: GitDiffKind,
    cx: &mut App,
) -> Task<Result<GitFileDiff>> {
    match cx
        .try_global::<GitDiffHost>()
        .context("Git diff access is unavailable")
    {
        Ok(host) => (host.0)(worktree_id, path, kind, cx),
        Err(error) => Task::ready(Err(error)),
    }
}
