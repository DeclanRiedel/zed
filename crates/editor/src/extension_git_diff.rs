use anyhow::{Context as _, Result};
use extension::git_diff::{GitDiffHost, GitDiffKind, GitFileDiff};
use gpui::{App, Task};
use project::ProjectPath;
use settings::WorktreeId;
use std::path::{Component, Path};
use util::{paths::PathStyle, rel_path::RelPath};
use workspace::AppState;

pub fn init(cx: &mut App) {
    cx.set_global(GitDiffHost(read));
}

fn read(
    worktree_id: u64,
    path: String,
    kind: GitDiffKind,
    cx: &mut App,
) -> Task<Result<GitFileDiff>> {
    let setup = (|| {
        anyhow::ensure!(
            !path.is_empty()
                && !path.contains('\\')
                && !path
                    .bytes()
                    .any(|byte| matches!(byte, b'\0' | b'\n' | b'\r'))
                && Path::new(&path)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
            "expected a worktree-relative file path without traversal or line breaks"
        );
        let path = RelPath::new(Path::new(&path), PathStyle::Unix)?;
        let worktree_id = WorktreeId::from_proto(worktree_id);
        let app_state = AppState::try_global(cx).context("no workspace is open")?;
        let project = app_state
            .workspace_store
            .read(cx)
            .workspaces()
            .filter_map(|workspace| workspace.upgrade())
            .map(|workspace| workspace.read(cx).project().clone())
            .find(|project| project.read(cx).worktree_for_id(worktree_id, cx).is_some())
            .context("worktree is no longer open")?;
        let git_store = project.read(cx).git_store().clone();
        let project_path = ProjectPath {
            worktree_id,
            path: path.into(),
        };
        anyhow::ensure!(
            git_store
                .read(cx)
                .repository_and_path_for_project_path(&project_path, cx)
                .is_some(),
            "file is not in a Git repository"
        );
        let buffer = project.update(cx, |project, cx| project.open_buffer(project_path, cx));
        Ok((git_store, buffer))
    })();
    let (git_store, buffer) = match setup {
        Ok(setup) => setup,
        Err(error) => return Task::ready(Err(error)),
    };
    cx.spawn(async move |cx| {
        let buffer = buffer.await?;
        let (before_text, mut after_text) = match kind {
            GitDiffKind::Staged => {
                let (before_text, after_text) = git_store
                    .update(cx, |git_store, cx| git_store.read_staged_texts(buffer, cx))
                    .await?;
                (before_text, after_text.unwrap_or_default())
            }
            GitDiffKind::Unstaged => {
                let (before_text, after_text) = git_store
                    .update(cx, |git_store, cx| {
                        git_store.read_unstaged_texts(buffer, cx)
                    })
                    .await?;
                (before_text, after_text)
            }
        };
        let mut before_text = before_text.unwrap_or_default();
        text::LineEnding::normalize(&mut before_text);
        text::LineEnding::normalize(&mut after_text);
        Ok(GitFileDiff {
            before_text,
            after_text,
        })
    })
}
