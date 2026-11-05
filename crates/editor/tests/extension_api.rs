use editor::{Editor, test::editor_lsp_test_context::EditorLspTestContext};
use extension::ghost_text;
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
