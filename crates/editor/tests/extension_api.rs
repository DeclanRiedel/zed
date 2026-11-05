use editor::{Editor, test::editor_lsp_test_context::EditorLspTestContext};
use extension::{
    ghost_text,
    git_diff::{self, GitDiffKind},
};
use gpui::TestAppContext;
use language::{Language, LanguageConfig, LanguageMatcher};

async fn context(cx: &mut TestAppContext) -> EditorLspTestContext {
    let language = Language::new(
        LanguageConfig {
            name: "PracticeTest".into(),
            matcher: LanguageMatcher {
                path_suffixes: vec!["practice".into()],
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
        None,
    );
    let context = EditorLspTestContext::new(language, Default::default(), cx).await;
    let state = context
        .workspace
        .read_with(cx, |workspace, _| workspace.app_state().clone());
    cx.update(|cx| workspace::AppState::set_global(state, cx));
    context
}

#[gpui::test]
async fn ghost_text_is_display_only_and_resource_release_clears_it(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Ghost proof".into(), "é".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        assert!(session.set(1, "bad".into(), cx).is_err());
        session.set(2, " ghost\ntext".into(), cx).unwrap();
        assert_eq!(session.text(cx).unwrap(), "é");
        assert_eq!(
            scratch.update(cx, |editor, cx| editor.display_text(cx)),
            "é ghost\ntext"
        );
        session.clear(cx).unwrap();
        assert_eq!(
            scratch.update(cx, |editor, cx| editor.display_text(cx)),
            "é"
        );
        session.set(2, " cleared on release".into(), cx).unwrap();
    });
    drop(session);
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            scratch.update(cx, |editor, cx| editor.display_text(cx)),
            "é"
        )
    });
}

#[gpui::test]
async fn staged_and_unstaged_snapshots_are_separate_and_read_only(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    cx.set_head_text("committed\n");
    cx.set_index_text("staged\n");
    cx.set_state("unsaved working textˇ\n");
    let (worktree, path) = cx.update_buffer(|buffer, cx| {
        let file = buffer.file().unwrap();
        (
            file.worktree_id(cx).to_proto(),
            file.path().as_unix_str().to_owned(),
        )
    });
    let staged = cx
        .update(|_, cx| git_diff::read(worktree, path.clone(), GitDiffKind::Staged, cx))
        .await
        .unwrap();
    assert_eq!(staged.before_text, "committed\n");
    assert_eq!(staged.after_text, "staged\n");
    let unstaged = cx
        .update(|_, cx| git_diff::read(worktree, path.clone(), GitDiffKind::Unstaged, cx))
        .await
        .unwrap();
    assert_eq!(unstaged.before_text, "staged\n");
    assert_eq!(unstaged.after_text, "unsaved working text\n");
    println!(
        "staged: {:?} -> {:?}; unstaged: {:?} -> {:?}",
        staged.before_text, staged.after_text, unstaged.before_text, unstaged.after_text
    );
    cx.assert_index_text(Some("staged\n"));
    cx.assert_editor_state("unsaved working textˇ\n");
    assert!(
        cx.update(|_, cx| git_diff::read(worktree, "../outside".into(), GitDiffKind::Staged, cx))
            .await
            .is_err()
    );
    assert!(
        cx.update(|_, cx| git_diff::read(worktree, "/absolute".into(), GitDiffKind::Staged, cx))
            .await
            .is_err()
    );
    assert!(
        cx.update(|_, cx| git_diff::read(u64::MAX, path, GitDiffKind::Staged, cx))
            .await
            .is_err()
    );
}
