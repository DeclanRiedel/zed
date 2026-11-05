use editor::{Editor, test::editor_lsp_test_context::EditorLspTestContext};
use extension::{
    ghost_text,
    git_diff::{self, GitDiffKind},
};
use gpui::TestAppContext;
use language::{Language, LanguageConfig, LanguageMatcher};
use multi_buffer::ToOffset as _;
use theme::ActiveTheme as _;

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
async fn guided_typing_consumes_rows_and_shades_only_wrong_characters(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Typing proof".into(), "old line\n".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        let progress = session.start_guided_typing("hello\n".into(), cx).unwrap();
        assert_eq!(progress.remove_characters, 0);
        assert_eq!(session.text(cx).unwrap(), "");
    });
    for (input, text, display, error_range) in [
        ("h", "h", "hello\n", None),
        ("x", "hx", "hxllo\n", Some(1..2)),
    ] {
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| editor.handle_input(input, window, cx))
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| {
                assert_eq!(editor.text(cx), text);
                assert_eq!(editor.display_text(cx), display);
                let snapshot = editor.display_snapshot(cx);
                let caret = editor.selections.newest_display(&snapshot).head();
                assert_eq!(caret.row().0, 0);
                assert_eq!(caret.column(), text.len() as u32);
                let key = editor::HighlightKey::GuidedTypingError;
                if let Some(expected) = error_range {
                    let (style, ranges) = editor
                        .text_highlights(key, cx)
                        .expect("wrong character must be highlighted");
                    assert_eq!(style.color, Some(cx.theme().status().error));
                    assert_eq!(
                        style.background_color,
                        Some(cx.theme().status().error_background)
                    );
                    let buffer = editor.buffer().read(cx).snapshot(cx);
                    let actual: Vec<_> = ranges
                        .iter()
                        .map(|range| {
                            range.start.to_offset(&buffer).0..range.end.to_offset(&buffer).0
                        })
                        .collect();
                    assert_eq!(actual, vec![expected]);
                    let red = cx.theme().status().error;
                    let red_text: String = snapshot
                        .chunks(
                            editor::display_map::DisplayRow(0)..editor::display_map::DisplayRow(1),
                            language::LanguageAwareStyling {
                                tree_sitter: false,
                                diagnostics: false,
                            },
                            Default::default(),
                        )
                        .filter(|chunk| {
                            chunk
                                .highlight_style
                                .is_some_and(|style| style.color == Some(red))
                        })
                        .map(|chunk| {
                            assert!(
                                !chunk.is_inlay,
                                "remaining ghost text must not inherit error shading"
                            );
                            chunk.text
                        })
                        .collect();
                    assert_eq!(red_text, "x");
                } else {
                    assert!(editor.text_highlights(key, cx).is_none());
                }
                assert!(!editor.has_active_edit_prediction());
                editor.accept_edit_prediction(&editor::actions::AcceptEditPrediction, window, cx);
                assert_eq!(editor.text(cx), text);
                println!("typed={text:?}; display={display:?}; caret={caret:?}");
            })
        });
    }
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| {
            editor.backspace(&editor::actions::Backspace, window, cx)
        })
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        scratch.update(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "h");
            assert_eq!(editor.display_text(cx), "hello\n");
            assert!(
                editor
                    .text_highlights(editor::HighlightKey::GuidedTypingError, cx)
                    .is_none()
            );
        })
    });
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| editor.handle_input("ello\n", window, cx))
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(session.progress(cx).unwrap().complete);
        assert_eq!(session.text(cx).unwrap(), "hello\n");
        session.clear(cx).unwrap();
        assert!(session.progress(cx).is_err());
    });
    cx.assert_index_text(None);
}

#[gpui::test]
async fn typing_cursor_stays_before_remaining_ghost_text(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Caret proof".into(), String::new(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        session.start_guided_typing("hello\n".into(), cx).unwrap();
    });
    cx.update(|_, cx| {
        scratch.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            let caret = editor.selections.newest_display(&snapshot).head();
            assert_eq!(caret.row().0, 0, "caret must remain on the first ghost row");
            assert_eq!(caret.column(), 0);
        })
    });
    cx.update(|window, cx| scratch.update(cx, |editor, cx| editor.handle_input("h", window, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        scratch.update(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "h");
            assert_eq!(editor.display_text(cx), "hello\n");
            let snapshot = editor.display_snapshot(cx);
            let caret = editor.selections.newest_display(&snapshot).head();
            assert_eq!(caret.row().0, 0);
            assert_eq!(caret.column(), 1);
        })
    });
}

#[gpui::test]
async fn rendered_practice_consumes_platform_keyboard_input(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Keyboard proof".into(), "old line\n".into(), cx))
        .unwrap();
    cx.update(|_, cx| {
        session.start_guided_typing("hello\n".into(), cx).unwrap();
    });
    cx.cx.editor = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let initial_width = cx.cx.rendered_line_widths()[0];
    assert!(
        initial_width > gpui::px(0.),
        "ghost prompt must be rendered before typing"
    );
    cx.simulate_keystrokes("h x");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(cx.cx.rendered_line_widths()[0], initial_width);
    cx.update(|_, cx| {
        assert_eq!(session.text(cx).unwrap(), "hx");
        assert_eq!(session.progress(cx).unwrap().remove_characters, 1);
    });
    cx.workspace.update_in(&mut cx.cx.cx, |workspace, _, cx| {
        let scratch = workspace.active_item_as::<Editor>(cx).unwrap();
        scratch.update(cx, |editor, cx| {
            assert_eq!(editor.display_text(cx), "hxllo\n");
            assert!(
                editor
                    .text_highlights(editor::HighlightKey::GuidedTypingError, cx)
                    .is_some()
            );
        });
    });
}

#[gpui::test]
async fn multiline_prompt_does_not_shift_as_rows_are_typed(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Multiline proof".into(), "old\nstuff\n".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        session
            .start_guided_typing("alpha\nbeta\n".into(), cx)
            .unwrap();
    });
    for (input, display, row, column) in [
        ("alpha\n", "alpha\nbeta\n", 1, 0),
        ("bX", "alpha\nbXta\n", 1, 2),
    ] {
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| editor.handle_input(input, window, cx))
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            scratch.update(cx, |editor, cx| {
                assert_eq!(editor.display_text(cx), display);
                let snapshot = editor.display_snapshot(cx);
                let caret = editor.selections.newest_display(&snapshot).head();
                assert_eq!((caret.row().0, caret.column()), (row, column));
            })
        });
    }
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| {
            editor.backspace(&editor::actions::Backspace, window, cx)
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| editor.handle_input("eta\n", window, cx))
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(session.progress(cx).unwrap().complete));
}

#[gpui::test]
async fn enter_keeps_indented_prompt_at_column_one(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Enter proof".into(), "old\n".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        session
            .start_guided_typing("    one\n    two\n".into(), cx)
            .unwrap();
    });
    for line in ["    one", "    two"] {
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| editor.handle_input(line, window, cx))
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| {
                editor.newline(&editor::actions::Newline, window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            scratch.update(cx, |editor, cx| {
                assert!(editor.text(cx).ends_with('\n'));
                assert_eq!(editor.display_text(cx), "    one\n    two\n");
                let snapshot = editor.display_snapshot(cx);
                let caret = editor.selections.newest_display(&snapshot).head();
                assert_eq!(caret.column(), 0, "Enter must leave indentation for typing");
            })
        });
    }
    cx.update(|_, cx| assert!(session.progress(cx).unwrap().complete));
}

#[gpui::test]
async fn separate_changes_keep_context_and_start_at_column_one(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Hunk proof".into(), "old\nkeep\nbad\n".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        session
            .start_guided_typing("new\nkeep\ngood\n".into(), cx)
            .unwrap();
        assert_eq!(session.text(cx).unwrap(), "keep\n");
    });
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| editor.handle_input("new\n", window, cx))
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        scratch.update(cx, |editor, cx| {
            assert_eq!(editor.display_text(cx), "new\nkeep\ngood\n");
            let snapshot = editor.display_snapshot(cx);
            let caret = editor.selections.newest_display(&snapshot).head();
            assert_eq!((caret.row().0, caret.column()), (2, 0));
        })
    });
    cx.update(|window, cx| {
        scratch.update(cx, |editor, cx| editor.handle_input("good\n", window, cx))
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(session.progress(cx).unwrap().complete));
}

#[gpui::test]
async fn deletion_only_rows_are_removed_with_delete_from_row_start(cx: &mut TestAppContext) {
    let mut cx = context(cx).await;
    let session = cx
        .cx
        .cx
        .cx
        .update(|cx| ghost_text::open("Deletion proof".into(), "remove\nkeep\n".into(), cx))
        .unwrap();
    let scratch = cx.workspace.read_with(&cx.cx.cx, |workspace, cx| {
        workspace.active_item_as::<Editor>(cx).unwrap()
    });
    cx.update(|_, cx| {
        assert_eq!(
            session
                .start_guided_typing("keep\n".into(), cx)
                .unwrap()
                .remove_characters,
            7
        );
    });
    for _ in 0..7 {
        cx.update(|window, cx| {
            scratch.update(cx, |editor, cx| {
                editor.delete(&editor::actions::Delete, window, cx)
            })
        });
        cx.run_until_parked();
    }
    cx.update(|_, cx| {
        assert_eq!(session.text(cx).unwrap(), "keep\n");
        assert!(session.progress(cx).unwrap().complete);
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
