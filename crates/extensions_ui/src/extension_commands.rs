use std::sync::Arc;

use anyhow::{Context as _, Result};
use editor::Editor;
use extension::{Extension as _, SlashCommand, WorktreeDelegate};
use extension_host::ExtensionStore;
use gpui::{App, Context, Window};
use language_extension::WorktreeDelegateAdapter;
use project::ProjectItem as _;
use schemars::JsonSchema;
use serde::Deserialize;
use workspace::{Toast, Workspace, notifications::NotificationId};

/// Invokes a command declared in an extension's slash_commands manifest.
/// Arguments equal to "$FILE" expand to the active worktree-relative file path.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, JsonSchema, gpui::Action)]
#[action(namespace = extensions)]
#[serde(deny_unknown_fields)]
pub struct RunCommand {
    pub extension_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(run);
    })
    .detach();
}

fn run(
    workspace: &mut Workspace,
    action: &RunCommand,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let setup = (|| -> Result<_> {
        let store = ExtensionStore::global(cx);
        let (manifest, extension) = store
            .read(cx)
            .wasm_extensions
            .iter()
            .find(|(manifest, _)| manifest.id.as_ref() == action.extension_id)
            .context("extension is not loaded")?;
        let command = manifest
            .slash_commands
            .get(action.name.as_str())
            .context("extension does not declare this command")?;
        let command = SlashCommand {
            name: action.name.clone(),
            description: command.description.clone(),
            tooltip_text: command.description.clone(),
            requires_argument: command.requires_argument,
        };
        let extension = extension.clone();
        let project = workspace.project().clone();
        let active_path = workspace.active_item_as::<Editor>(cx).and_then(|editor| {
            editor
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .and_then(|buffer| buffer.read(cx).project_path(cx))
        });
        let arguments = action
            .arguments
            .iter()
            .map(|argument| {
                if argument == "$FILE" {
                    active_path
                        .as_ref()
                        .map(|path| path.path.as_unix_str().to_owned())
                        .context("active editor is not a project file")
                } else {
                    Ok(argument.clone())
                }
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            !command.requires_argument || !arguments.is_empty(),
            "command requires arguments"
        );
        let worktree = active_path
            .as_ref()
            .and_then(|path| project.read(cx).worktree_for_id(path.worktree_id, cx))
            .or_else(|| project.read(cx).visible_worktrees(cx).next());
        let worktree = match worktree {
            Some(worktree) => {
                let lsp_store = project.read(cx).lsp_store();
                let delegate = lsp_store.update(cx, |store, cx| {
                    store.worktree_adapter_delegate(&worktree, cx)
                })?;
                Some(Arc::new(WorktreeDelegateAdapter(delegate)) as Arc<dyn WorktreeDelegate>)
            }
            None => None,
        };
        Ok((extension, command, arguments, worktree))
    })();
    let (extension, command, arguments, worktree) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            workspace.show_error(error, cx);
            return;
        }
    };
    cx.spawn_in(window, async move |workspace, cx| {
        let result = extension
            .run_slash_command(command, arguments, worktree)
            .await;
        workspace
            .update_in(cx, |workspace, _, cx| match result {
                Ok(output) => workspace.show_toast(
                    Toast::new(NotificationId::unique::<RunCommand>(), output.text),
                    cx,
                ),
                Err(error) => workspace.show_error(error, cx),
            })
            .map_err(|error| log::debug!("extension command workspace closed: {error}"))
            .unwrap_or_default();
    })
    .detach();
}
