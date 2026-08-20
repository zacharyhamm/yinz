use std::{collections::BTreeMap, path::PathBuf};

use hocus_proto::{
    agents::{self, AgentPane},
    wire::{self, PollReply},
};
use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{
    Button, Checkbox, Dialog, Form, FormEvent, Rect, Select, TextInput,
};

const AGENT: &str = "agent";
const MODEL: &str = "model";
const CREATE_BRANCH: &str = "create_branch";
const BASE: &str = "base";
const BRANCH: &str = "branch";
const WORKTREE: &str = "worktree";
const PATH: &str = "path";
const DUPLICATE_FOCUSED: &str = "duplicate_focused";
const CALLER_TERMINAL_PANE_ID: &str = "caller_terminal_pane_id";

struct State {
    dialog: Dialog,
    source_directory: PathBuf,
    plugin_id: u32,
    duplicate_pane_id: Option<u32>,
    path_edited: bool,
    error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        let form = Form::new()
            .select(AGENT, "Agent", Select::new(["Codex", "Claude", "Cursor"]))
            .field(MODEL, "Model (optional)", TextInput::empty())
            .checkbox(
                CREATE_BRANCH,
                "Create and switch branch",
                Checkbox::new(true),
            )
            .field(BASE, "Base", TextInput::new("main".to_owned()))
            .field(BRANCH, "Branch", TextInput::empty())
            .checkbox(WORKTREE, "Use worktree", Checkbox::new(true))
            .field(
                PATH,
                "Worktree path",
                TextInput::new(".worktrees/".to_owned()),
            )
            .button("create", Button::new("Create agent"));
        Self {
            dialog: Dialog::new(Text::new("New agent").color_all(2), form)
                .help(Text::new("<Tab> next  <Enter> create  <Esc> cancel")),
            source_directory: PathBuf::from("."),
            plugin_id: 0,
            duplicate_pane_id: None,
            path_edited: false,
            error: None,
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.source_directory = configuration
            .get("caller_cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.plugin_id = get_plugin_ids().plugin_id;
        if configuration.get(DUPLICATE_FOCUSED).map(String::as_str) == Some("true") {
            self.duplicate_pane_id = configuration
                .get(CALLER_TERMINAL_PANE_ID)
                .and_then(|pane_id| pane_id.parse().ok());
            if self.duplicate_pane_id.is_some() {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            }
        }
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::TabUpdate,
            EventType::PastedText,
            EventType::Visible,
        ]);
        set_selectable(true);
        rename_plugin_pane(self.plugin_id, "new agent");
        self.resize(None);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::TabUpdate(tabs) => {
                if let Some(tab) = tabs.first() {
                    self.resize(Some(tab.display_area_columns));
                }
                false
            },
            Event::Key(key) => {
                let event = self.dialog.handle_key(key);
                self.handle(event)
            },
            Event::Mouse(mouse) => {
                let event = self.dialog.handle_mouse(mouse);
                self.handle(event)
            },
            Event::PastedText(text) => {
                let focused = self.dialog.form().focused_id().map(str::to_owned);
                if let Some(input) = focused
                    .as_deref()
                    .and_then(|id| self.dialog.form_mut().field_by_id_mut(id))
                    .and_then(|field| field.input_mut())
                {
                    input.insert_text(&text);
                    self.changed(focused.as_deref().unwrap_or_default());
                    true
                } else {
                    false
                }
            },
            Event::Visible(false) => {
                close_self();
                false
            },
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.dialog.set_message(
            self.error
                .as_ref()
                .map(|error| Text::new(error).color_all(3)),
        );
        self.dialog.render(Rect::new(0, 0, cols, rows));
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_HELLO && self.duplicate_pane_id.is_some() {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            return false;
        }
        if pipe_message.name != wire::AGENT_SNAPSHOT {
            return false;
        }
        let Some(reply) = pipe_message
            .payload
            .as_deref()
            .and_then(|payload| serde_json::from_str::<PollReply>(payload).ok())
        else {
            return false;
        };
        let Some(pane_id) = self.duplicate_pane_id.take() else {
            return false;
        };
        let command = reply
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .and_then(|pane| duplicate_command(pane, &reply.manual_kinds));
        if command.is_some_and(|command| {
            open_command_pane_near_plugin(
                command,
                BTreeMap::from([(HOCUS_AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
            )
            .is_some()
        }) {
            close_self();
        }
        false
    }
}

fn modal_width(display_columns: Option<usize>) -> String {
    display_columns
        .map(|columns| columns.div_ceil(2).min(60).to_string())
        .unwrap_or_else(|| "50%".to_owned())
}

impl State {
    fn resize(&self, display_columns: Option<usize>) {
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            None,
            Some("20%".into()),
            Some(modal_width(display_columns)),
            Some("14".into()),
            None,
            None,
            None,
        ) {
            change_floating_panes_coordinates(vec![(
                PaneId::Plugin(self.plugin_id),
                coordinates.as_overlay(),
            )]);
        }
    }
}

fn duplicate_command(
    pane: &AgentPane,
    manual_kinds: &BTreeMap<u32, String>,
) -> Option<CommandToRun> {
    let (kind, _) = agents::classify(pane, manual_kinds)?;
    Some(CommandToRun {
        path: if kind == "cursor" {
            "cursor-agent".into()
        } else {
            kind.into()
        },
        // `/host` is translated to the plugin's launch-time caller CWD, without
        // reinterpreting a real `/tmp`, `/host`, `/data` or `/cache` path.
        cwd: Some("/host".into()),
        remote_host: pane.host.clone(),
        ..Default::default()
    })
}

impl State {
    fn handle(&mut self, event: FormEvent) -> bool {
        match event {
            FormEvent::Submit | FormEvent::ButtonPressed(_) => self.submit(),
            FormEvent::Cancel => {
                close_self();
                false
            },
            FormEvent::FieldChanged(id) => {
                self.changed(&id);
                true
            },
            other => other.should_render(),
        }
    }

    fn changed(&mut self, id: &str) {
        self.error = None;
        if id == CREATE_BRANCH && !self.dialog.form().checked(CREATE_BRANCH) {
            self.set_checked(WORKTREE, false);
        }
        if id == WORKTREE && self.dialog.form().checked(WORKTREE) {
            self.set_checked(CREATE_BRANCH, true);
        }
        if id == PATH {
            self.path_edited = true;
        }
        if id == BRANCH && !self.path_edited {
            let branch = self.dialog.form().text(BRANCH).to_owned();
            if let Some(input) = self
                .dialog
                .form_mut()
                .field_by_id_mut(PATH)
                .and_then(|field| field.input_mut())
            {
                input.set_text(format!(".worktrees/{branch}"));
            }
        }
    }

    fn set_checked(&mut self, id: &str, checked: bool) {
        if let Some(checkbox) = self.dialog.form_mut().checkbox_by_id_mut(id) {
            checkbox.set_checked(checked);
        }
    }

    fn submit(&mut self) -> bool {
        let (command, checkout) = match request(self.dialog.form()) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(error);
                return true;
            },
        };
        match open_command_pane_in_git_checkout_near_plugin(
            command,
            self.source_directory.clone(),
            checkout,
        ) {
            Ok(_) => {
                close_self();
                false
            },
            Err(error) => {
                self.error = Some(error);
                true
            },
        }
    }
}

fn request(form: &Form) -> Result<(CommandToRun, GitCheckout), String> {
    let executable = match form.text(AGENT) {
        "Codex" => "codex",
        "Claude" => "claude",
        "Cursor" => "cursor-agent",
        _ => return Err("choose an agent".to_owned()),
    };
    let model = form.text(MODEL).trim();
    let args = if model.is_empty() {
        vec![]
    } else {
        vec!["--model".to_owned(), model.to_owned()]
    };
    let command = CommandToRun {
        path: executable.into(),
        args,
        ..Default::default()
    };
    let checkout = if !form.checked(CREATE_BRANCH) {
        GitCheckout::Current
    } else {
        let branch = form.text(BRANCH).trim();
        let base = form.text(BASE).trim();
        if branch.is_empty() || base.is_empty() {
            return Err("branch and base are required".to_owned());
        }
        if form.checked(WORKTREE) {
            let path = form.text(PATH).trim();
            if path.is_empty() {
                return Err("worktree path is required".to_owned());
            }
            GitCheckout::NewWorktree {
                branch: branch.to_owned(),
                base: base.to_owned(),
                path: path.into(),
            }
        } else {
            GitCheckout::NewBranch {
                branch: branch.to_owned(),
                base: base.to_owned(),
            }
        }
    };
    Ok((command, checkout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_is_half_the_display_up_to_sixty_columns() {
        assert_eq!(modal_width(Some(80)), "40");
        assert_eq!(modal_width(Some(200)), "60");
    }

    #[test]
    fn defaults_create_a_codex_worktree_from_main() {
        let mut state = State::default();
        state
            .dialog
            .form_mut()
            .field_by_id_mut(BRANCH)
            .unwrap()
            .input_mut()
            .unwrap()
            .set_text("topic".into());
        state.changed(BRANCH);
        let (command, checkout) = request(state.dialog.form()).unwrap();
        assert_eq!(command.path, PathBuf::from("codex"));
        assert!(
            matches!(checkout, GitCheckout::NewWorktree { branch, base, path }
            if branch == "topic" && base == "main" && path == PathBuf::from(".worktrees/topic"))
        );
    }

    #[test]
    fn model_is_passed_directly_to_the_selected_agent() {
        let mut state = State::default();
        state.set_checked(CREATE_BRANCH, false);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .select_option("Claude");
        state
            .dialog
            .form_mut()
            .field_by_id_mut(MODEL)
            .unwrap()
            .input_mut()
            .unwrap()
            .set_text("opus".into());
        let (command, checkout) = request(state.dialog.form()).unwrap();
        assert_eq!(command.path, PathBuf::from("claude"));
        assert_eq!(command.args, ["--model", "opus"]);
        assert_eq!(checkout, GitCheckout::Current);
    }

    #[test]
    fn duplicate_uses_the_agent_kind_and_exact_location_without_old_args() {
        let pane = AgentPane {
            pane_id: 7,
            title: "agent".to_owned(),
            command: Some("git status".to_owned()),
            cwd: Some("/tmp/repo".to_owned()),
            host: Some("builder".to_owned()),
            status: agents::AgentStatus::Running,
            tab: Some((1, "work".to_owned())),
            detected: Some("cursor".to_owned()),
        };
        let command = duplicate_command(&pane, &BTreeMap::new()).unwrap();

        assert_eq!(command.path, PathBuf::from("cursor-agent"));
        assert!(command.args.is_empty());
        assert_eq!(command.cwd, Some("/host".into()));
        assert_eq!(command.remote_host.as_deref(), Some("builder"));

        let ordinary_pane = AgentPane {
            command: Some("bash".to_owned()),
            detected: None,
            ..pane
        };
        assert!(duplicate_command(&ordinary_pane, &BTreeMap::new()).is_none());
    }
}
