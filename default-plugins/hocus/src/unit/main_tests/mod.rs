use super::*;
use hocus_proto::notes;
use hocus_proto::panes::terminal_panes;
use yinz_tile::prelude::HostId;
use yinz_tile::prelude::TabId;
use yinz_tile::prelude::TabPosition;

fn state_with(messages: Vec<Message>, status: Status) -> State {
    State {
        messages,
        status,
        ..State::default()
    }
}

/// Mid-reply, as `start_request` would have left it: a stream owning `id`,
/// and the trailing assistant message as the placeholder it appends into.
fn state_streaming(messages: Vec<Message>, id: &str) -> State {
    let reply_at = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant);
    State {
        messages,
        reply_at,
        stream: Some(StreamState {
            id: id.to_owned(),
            finishing: false,
            usage: None,
        }),
        // The overlay panel, as `ready_state` is - see there.
        own_is_floating: Some(true),
        ..State::default()
    }
}

/// Everything outstanding has landed — the reply and the tool queue both.
/// The two are independent now, so "idle" takes clearing both.
fn settle(state: &mut State) {
    state.status = Status::Idle;
    state.stream = None;
    state.reply_at = None;
}

/// The stream that has already had its outcome recorded and is only waiting
/// to swallow the host's last chunk.
fn finishing(id: &str) -> Option<StreamState> {
    Some(StreamState {
        id: id.to_owned(),
        finishing: true,
        usage: None,
    })
}

/// A state that can actually send, so the tool loop reaches its follow-up
/// request. `web_request_stream` is a no-op stub off wasm, so nothing leaves.
fn ready_state() -> State {
    State {
        perms: true,
        // The overlay panel, which is what hocus is everywhere but a user's own layout. Set here
        // because it is what `is_panel` reads, and without it Esc, `show_self` and every other
        // panel gesture below are correctly refused - see `State::own_is_floating`.
        own_is_floating: Some(true),
        // Every per-pane row this plugin writes is keyed by host and session as well as pane
        // id, so a state that has never heard a snapshot writes none. Standing in for the
        // first one. The host is named rather than left empty so that a helper which forgot to
        // write it fails a test instead of matching the pre-migration rows.
        yinz_session_name: "test-session".to_owned().into(),
        host_id: HostId::new("test-host"),
        cfg: Config::resolve(
            &BTreeMap::new(),
            &BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())]),
        ),
        stream: Some(StreamState {
            id: "hocus-1".into(),
            finishing: false,
            usage: None,
        }),
        reply_at: Some(1),
        messages: vec![Message::user("hi"), Message::assistant("")],
        ..State::default()
    }
}

/// One SSE frame carrying a complete tool call, as `on_chunk` would see it.
fn tool_call_frame(id: &str, name: &str, arguments: &str) -> Vec<u8> {
    format!(
        r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"{id}","function":{{"name":"{name}","arguments":"{}"}}}}]}}}}]}}
"#,
        arguments.replace('"', "\\\"")
    )
    .into_bytes()
}

fn tool_results(state: &State) -> Vec<&Message> {
    state
        .messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect()
}

fn tool_result_index(state: &State) -> usize {
    state
        .messages
        .iter()
        .position(|message| message.role == Role::Tool)
        .expect("a tool result")
}

// -- skills -------------------------------------------------------------

/// Load one skill into a tier, as a scan would have. Nothing is reserved,
/// so a test can name it after a tool on purpose.
fn load_skill(state: &mut State, source: skills::Source, name: &str, body: &str) {
    let path = format!("/skills/{name}/SKILL.md");
    let listing = skills::parse_listing(
        source,
        &format!("\u{1}{path}\u{2}---\ndescription: The {name} skill.\n---\n{body}\n"),
    );
    state
        .skills
        .replace(source, listing, &std::collections::BTreeSet::new());
}

// -- the file tools -----------------------------------------------------

/// A state whose active tab is a project, which is what the file tools read
/// from. `root` is never touched: nothing here runs a command.
fn project_state() -> State {
    let mut state = ready_state();
    state.projects.insert(
        "hocus".to_owned(),
        projects::Project {
            name: "hocus".to_owned(),
            root: "/work/hocus".to_owned(),
            firecracker_image: None,
            worktree_dir: None,
            remote: None,
            worktree_status: None,
            common_dir: None,
            discovered: false,
            replaced_ids: Vec::new(),
        },
    );
    state.active_project = Some("hocus".to_owned());
    state
}

fn run_file_tool(state: &mut State, name: &str, arguments: &str) -> bool {
    let call = ToolCall {
        id: "mine".into(),
        name: name.into(),
        arguments: arguments.into(),
    };
    state.pending = vec![call.clone()].into();
    state.run_head(&call)
}

// -- web_search ---------------------------------------------------------

fn searching_state(query: &str) -> State {
    let mut state = ready_state();
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "web_search".into(),
        arguments: format!(r#"{{"query":"{query}"}}"#),
    }]
    .into();
    state
}

// -- the archive --------------------------------------------------------

fn picker_state(titles: &[&str]) -> State {
    State {
        picker: Some(Picker::new(
            titles.iter().map(|title| PickerRow::item(*title)).collect(),
            0,
            PickerKind::Chats((1..=titles.len() as u64).collect()),
        )),
        ..ready_state()
    }
}

fn picker(state: &State) -> &Picker {
    state.picker.as_ref().expect("the picker is open")
}

/// The open picker's rows as plain lines, which is all most of these tests
/// care about.
fn entry_texts(state: &State) -> Vec<&str> {
    picker(state)
        .entries
        .iter()
        .map(|row| row.text.as_str())
        .collect()
}

/// The rows the picker's filter leaves, which is what is drawn.
fn visible_texts(state: &State) -> Vec<&str> {
    let picker = picker(state);
    picker
        .visible()
        .into_iter()
        .map(|at| picker.entries[at].text.as_str())
        .collect()
}

/// Type into the open picker's filter, a key at a time, as the user would.
fn type_filter(state: &mut State, text: &str) {
    for c in text.chars() {
        state.picker_key(KeyWithModifier::new(BareKey::Char(c)));
    }
}

/// The agent strip as it would be pushed to the hocus-agents pane, as
/// plain lines.
fn agent_entry_texts(state: &State) -> Vec<String> {
    state
        .current_strip()
        .rows
        .into_iter()
        .map(|row| match row {
            agents::StripRow::Project(text) | agents::StripRow::Branch(text) => text,
            agents::StripRow::Agent { text, .. } => text,
            // The archive is a table in the pane, so a run has no one line here. Composed the
            // way the pane composes it, minus the widths, which is what these tests read.
            agents::StripRow::Run {
                kind,
                title,
                continuation,
                ..
            } => format!("{}{kind} · {title}", if continuation { "↳ " } else { "" }),
            agents::StripRow::MoreRuns => "load older runs…".to_owned(),
        })
        .collect()
}

// -- the model list -----------------------------------------------------

/// Idle, with the picker open and the fetch in flight — the state `/models`
/// leaves behind. `web_request` is a no-op stub off wasm.
fn fetching_models() -> State {
    let mut state = ready_state();
    settle(&mut state);
    state.run_manual("/models");
    state
}

/// One SSE frame of reply text, as `on_chunk` would see it.
fn delta_frame(text: &str) -> Vec<u8> {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n").into_bytes()
}

/// What the host hands back on the response that leads a chat stream.
fn stream_context(id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(STREAM_CONTEXT.to_owned(), id.to_owned())])
}

fn models_context() -> BTreeMap<String, String> {
    BTreeMap::from([("models".to_owned(), "1".to_owned())])
}

/// Two agents in two worktrees of one repository — the case the branch
/// headers exist for. Nothing names the project, so they group under the
/// header that says so and are still told apart by their branches.
fn state_with_worktrees() -> State {
    let mut state = idle_state();
    state.set_panes(vec![
        agent_in(3, "/work/hocus", TabId::new(1)),
        agent_in(5, "/work/trees/topic", TabId::new(2)),
    ]);
    state.repositories = BTreeMap::from([
        (
            Location::local("/work/hocus"),
            on_branch("/work/hocus", "main"),
        ),
        (
            Location::local("/work/trees/topic"),
            on_branch("/work/trees/topic", "topic"),
        ),
    ]);
    state
}

/// The daemon's snapshot of the two worktree agents, with pane 3 moved to
/// `branch`.
fn worktree_snapshot(seq: u64, branch: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(1)),
        name: wire::AGENT_SNAPSHOT.to_owned(),
        payload: Some(
            serde_json::to_string(&PollReply {
                suppressed_panes: Vec::new(),
                panes: Some(vec![
                    agent_in(3, "/work/hocus", TabId::new(1)),
                    agent_in(5, "/work/trees/topic", TabId::new(2)),
                ]),
                activity: agents::Activity::new(),
                manual_kinds: BTreeMap::new(),
                yinz_session_name: "test-session".to_owned().into(),
                tab_projects: BTreeMap::new(),
                repositories: BTreeMap::from([
                    (
                        Location::local("/work/hocus"),
                        on_branch("/work/hocus", branch),
                    ),
                    (
                        Location::local("/work/trees/topic"),
                        on_branch("/work/trees/topic", "topic"),
                    ),
                ]),
                focused: None,
                summaries: agents::Summaries::new(),
                hooks: agents::HookFeeds::new(),
                runs: Vec::new(),
            })
            .expect("serialize"),
        ),
        args: BTreeMap::from([("seq".to_owned(), seq.to_string())]),
        is_private: true,
        pane_id: None,
    }
}

// -----------------------------------------------------------------------
// Scheduled jobs
// -----------------------------------------------------------------------

fn idle_state() -> State {
    State {
        status: Status::Idle,
        stream: None,
        reply_at: None,
        messages: Vec::new(),
        ..ready_state()
    }
}

#[test]
fn mode_updates_keep_kitty_graphics_support_per_client() {
    let mut state = idle_state();
    let mode_update = |supported| {
        let mut mode_info = ModeInfo::default();
        mode_info.capabilities.kitty_graphics = supported;
        Event::ModeUpdate(mode_info)
    };

    state.client = ClientId::new(1);
    assert!(state.handle(mode_update(true)));
    state.client = ClientId::new(2);
    assert!(!state.handle(mode_update(false)));
    assert_eq!(state.kitty_graphics, BTreeSet::from([ClientId::new(1)]));

    assert!(state.handle(mode_update(true)));
    state.client = ClientId::new(1);
    assert!(state.handle(mode_update(false)));
    assert_eq!(state.kitty_graphics, BTreeSet::from([ClientId::new(2)]));
}

fn schedule_call(arguments: &str) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: "schedule".into(),
        arguments: arguments.to_owned(),
    }
}

/// An agent pane in a folder, in a tab, as a poll would report it.
fn agent_in(pane_id: u32, cwd: &str, tab_id: TabId) -> agents::AgentPane {
    agents::AgentPane {
        pane_id,
        title: "codex".to_owned(),
        command: Some("codex".to_owned()),
        cwd: Some(cwd.to_owned()),
        host: None,
        firecracker_image: None,
        firecracker_worktree: None,
        status: agents::AgentStatus::Running,
        tab: Some((tab_id, format!("tab {tab_id}"))),
        detected: None,
        agent: None,
    }
}

/// An agent in a Firecracker VM, whose folder is a guest path in a copy of
/// `worktree` — the folder on this machine its work is merged back into.
fn vm_agent_in(pane_id: u32, worktree: &str, tab_id: TabId) -> agents::AgentPane {
    agents::AgentPane {
        host: Some("firecracker:review".to_owned()),
        firecracker_worktree: Some(worktree.to_owned()),
        ..agent_in(pane_id, "/workspace", tab_id)
    }
}

/// A plain shell, which names a tab but is nobody's agent.
fn shell_in(pane_id: u32, cwd: &str, tab_id: TabId) -> agents::AgentPane {
    agents::AgentPane {
        title: "zsh".to_owned(),
        command: Some("zsh".to_owned()),
        ..agent_in(pane_id, cwd, tab_id)
    }
}

/// A state with one tab, active, that the poll can assign.
fn state_with_tab(tab_id: TabId) -> State {
    let mut state = idle_state();
    state.tabs = vec![TabInfo {
        active: true,
        tab_id,
        ..TabInfo::default()
    }];
    state
}

/// A folder Git does not claim, as the daemon's probe would report it.
fn folder(root: &str) -> projects::Repository {
    projects::Repository::folder(root)
}

/// A folder Git claims, as the daemon's probe would report it.
fn checkout(root: &str) -> projects::Repository {
    on_branch(root, "main")
}

/// The same, with the branch it has out.
fn on_branch(root: &str, branch: &str) -> projects::Repository {
    projects::Repository {
        root: root.to_owned(),
        remote: None,
        worktree_status: Some(projects::WorktreeStatus::Primary),
        common_dir: Some(format!("{root}/.git")),
        branch: Some(branch.to_owned()),
        host: None,
    }
}

/// A linked worktree of `primary`, as the daemon's probe would report it.
///
/// Its own root and its own branch, sharing the primary's git directory —
/// which is what makes the two one project and the branch the only thing
/// telling their agents apart.
fn worktree_of(root: &str, primary: &str, branch: &str) -> projects::Repository {
    projects::Repository {
        root: root.to_owned(),
        remote: None,
        worktree_status: Some(projects::WorktreeStatus::Linked),
        common_dir: Some(format!("{primary}/.git")),
        branch: Some(branch.to_owned()),
        host: None,
    }
}

/// Where this repository really keeps its worktrees: inside the checkout
/// they came from. Git puts them wherever it is asked to, and under the
/// primary is an ordinary place to ask for.
const NESTED_WORKTREE: &str = "/work/hocus/.claude/worktrees/topic";

/// One turn of the whole loop: the daemon's snapshot reaches hocus *and*
/// hocus-store, the store discovers projects from it and pushes the registry
/// back. Discovery lives in the store now, so a test that only called
/// `apply_poll` would be testing half a circuit.
///
/// Every agent folder is a checkout here, which is what makes a project.
/// The tests about folders outside a repository build their own.
fn poll(state: &mut State, panes: Vec<agents::AgentPane>) {
    poll_with(state, panes, checkout)
}

/// The same turn, with the daemon's Git verdict for each agent folder
/// chosen by the caller.
fn poll_with(
    state: &mut State,
    panes: Vec<agents::AgentPane>,
    probe: fn(&str) -> projects::Repository,
) {
    poll_focus(state, panes, probe, None)
}

/// The same turn, with the pane the daemon says is focused.
fn poll_focused(state: &mut State, panes: Vec<agents::AgentPane>, focused: Option<u32>) {
    poll_focus(state, panes, checkout, focused)
}

/// One daemon snapshot, in the order `pipe` applies one: the folders it
/// resolved, then the panes, then whatever the store made of them.
fn poll_focus(
    state: &mut State,
    panes: Vec<agents::AgentPane>,
    probe: fn(&str) -> projects::Repository,
    focused: Option<u32>,
) {
    // Every pane's folder, not only the agents' — the daemon keys probe retention
    // on `pane_locations()`, so a shell's checkout is probed too. Discovery below
    // is still fed the agents' folders alone, which is the same split the session
    // has: what is *probed* is wider than what is *remembered*.
    let probed: Vec<(Location, projects::Repository)> = panes
        .iter()
        .filter_map(|pane| {
            // The folder the daemon would have probed, which for a Firecracker pane is
            // the host worktree its VM mirrors rather than the guest copy.
            let location = pane.checkout_location()?;
            let repository = probe(&location.path).on_host(location.host.clone());
            Some((location, repository))
        })
        .collect();
    // Keyed by the pane's folder, which is what the daemon keys it by —
    // `finish_git_probe` inserts under the folder it asked about, not under
    // the checkout root that answered. The two are the same folder only when
    // a pane sits at the top of its checkout, and a fixture that always put
    // them there was the reason the stand-in path below was never driven the
    // way a session drives it.
    state.repositories = probed.iter().cloned().collect();
    let agent_folders: BTreeSet<Location> = panes
        .iter()
        .filter(|pane| agents::classify(pane, &state.manual_kinds).is_some())
        .filter_map(|pane| pane.checkout_location())
        .collect();
    let repositories: Vec<projects::Repository> = probed
        .into_iter()
        .filter(|(location, _)| agent_folders.contains(location))
        .map(|(_, repository)| repository)
        .collect();
    state.apply_poll(
        panes,
        agents::Activity::new(),
        agents::Summaries::new(),
        agents::HookFeeds::new(),
        focused,
    );
    // The snapshot handler pushes the strip on every snapshot, not only the
    // ones that move a project; without this the helper would only ever push
    // through `give_projects` below.
    state.push_strip(false);
    let mut registry = state.projects.clone();
    if projects::discover(&mut registry, &repositories).changed() {
        give_projects(state, registry);
    }
}

/// Deliver a project-rows change, as the store's watch would.
///
/// Row-shaped rather than registry-shaped on purpose: this is the same entry
/// point a real notification arrives at, so what the tests drive is what a
/// session runs. The rows are built from a registry because that is what the
/// callers have.
fn give_projects(state: &mut State, registry: projects::Registry) {
    let backend = state.store();
    let before = projects::from_rows(&backend.fetch(&FetchRequest::entity("project")).unwrap());
    backend
        .apply(projects::diff_mutations(&before, &registry))
        .unwrap();
    let rows = backend
        .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
        .unwrap();
    drop(backend);
    state.on_projects_changed(state.project_watch, &rows);
}

// -- todo lists ---------------------------------------------------------

/// A state whose active project is `hocus` at `/work/hocus`, assigned by
/// hand, so the todo tools have somewhere to write.
fn state_with_project() -> State {
    let mut state = state_with_tab(TabId::new(9));
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"hocus","root":"/work/hocus"}"#,
    ));
    state
}

/// Run a todo tool the way the whole system does.
///
/// Reads are answered from hocus's cached copy. Writes go out to the store
/// and come back, which is modelled here by applying the change to the same
/// pure functions hocus-store uses and pushing the result back as a
/// snapshot — so these tests still cover hocus's half of every call.
fn todo(state: &mut State, name: &str, arguments: &str) -> serde_json::Value {
    let result =
        todo_result(state, name, arguments).unwrap_or_else(|| panic!("{name} answered nothing"));
    serde_json::from_str(&result)
        .unwrap_or_else(|_| panic!("{name} answered with an error: {result}"))
}

/// The raw answer of a todo tool, or `None` if it is not one.
///
/// This used to simulate hocus-store: it re-ran the tree operation against a
/// copy of the todo map and fed the result back as a snapshot. That was a
/// second implementation of the tool, and it had drifted — it answered
/// `also_done` where the store answered `cascaded`. Now the tool runs for
/// real against the temporary store, so there is nothing left to drift.
fn todo_result(state: &mut State, name: &str, arguments: &str) -> Option<String> {
    let call = agent_call(name, arguments);
    if !matches!(name, "todo_add" | "todo_done" | "todo_edit") {
        return state.run_local(&call);
    }
    Some(match state.todo_write(state.store(), &call) {
        Ok(message) => message,
        Err(error) => error,
    })
}

/// One project's todo list, read back out of the store.
fn stored_list(state: &State, root: &str) -> todos::List {
    todos::fetch_list(&state.store(), root).expect("fetched")
}

// -- notes --------------------------------------------------------------

/// Run a note tool the way the whole system does: writes through
/// `note_write`, reads through `run_local`, both against the temporary store.
fn note(state: &mut State, name: &str, arguments: &str) -> serde_json::Value {
    let result =
        note_result(state, name, arguments).unwrap_or_else(|| panic!("{name} answered nothing"));
    serde_json::from_str(&result)
        .unwrap_or_else(|_| panic!("{name} answered with an error: {result}"))
}

/// The raw answer of a note tool, or `None` if it is not one.
fn note_result(state: &mut State, name: &str, arguments: &str) -> Option<String> {
    let call = agent_call(name, arguments);
    if !matches!(
        name,
        "note_add" | "note_edit" | "note_archive" | "note_delete" | "note_link"
    ) {
        return state.run_local(&call);
    }
    Some(match state.note_write(state.store(), &call) {
        Ok(message) => message,
        Err(error) => error,
    })
}

/// One note, read back out of the store.
fn stored_note(state: &State, id: u64) -> Option<notes::Note> {
    notes::fetch_note(&state.store(), id).expect("fetched")
}

// -- agents -------------------------------------------------------------

fn pane_info(id: u32, title: &str, command: Option<&str>) -> PaneInfo {
    PaneInfo {
        id: yinz_tile::prelude::PaneId::from_legacy(id, false),
        title: title.to_owned(),
        terminal_command: command.map(str::to_owned),
        ..PaneInfo::default()
    }
}

fn manifest(panes: Vec<PaneInfo>) -> PaneManifest {
    PaneManifest {
        panes: std::collections::HashMap::from([(TabPosition::new(0), panes)]),
    }
}

fn tab_info(position: usize, tab_id: TabId, name: &str) -> TabInfo {
    TabInfo {
        position: TabPosition::new(position),
        tab_id,
        name: name.to_owned(),
        ..TabInfo::default()
    }
}

/// A state that has seen one `codex` pane and one plain shell.
fn state_with_panes() -> State {
    let mut state = idle_state();
    state.set_panes(terminal_panes(
        &manifest(vec![
            pane_info(3, "codex", Some("codex --resume")),
            pane_info(4, "zsh", Some("zsh")),
        ]),
        &[],
    ));
    state
}

/// The same panes `state_with_panes` starts from, to refill after a
/// snapshot has emptied them.
fn panes_with_agent() -> Vec<agents::AgentPane> {
    terminal_panes(
        &manifest(vec![
            pane_info(3, "codex", Some("codex --resume")),
            pane_info(4, "zsh", Some("zsh")),
        ]),
        &[],
    )
}

fn agent_call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

/// An empty snapshot carrying `seq`, as hocus-agentd sends it.
fn agent_snapshot(seq: u64) -> PipeMessage {
    agent_snapshot_with(seq, Vec::new())
}

/// A snapshot carrying `seq` and the panes it saw.
fn agent_snapshot_with(seq: u64, panes: Vec<agents::AgentPane>) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(1)),
        name: wire::AGENT_SNAPSHOT.to_owned(),
        payload: Some(
            serde_json::to_string(&PollReply {
                suppressed_panes: Vec::new(),
                panes: Some(panes),
                activity: agents::Activity::new(),
                manual_kinds: BTreeMap::new(),
                yinz_session_name: "test-session".to_owned().into(),
                tab_projects: BTreeMap::new(),
                repositories: BTreeMap::new(),
                focused: None,
                summaries: agents::Summaries::new(),
                hooks: agents::HookFeeds::new(),
                runs: Vec::new(),
            })
            .expect("serialize"),
        ),
        args: BTreeMap::from([("seq".to_owned(), seq.to_string())]),
        is_private: true,
        pane_id: None,
    }
}

/// The two worktree agents as a daemon snapshot, with `focused` moved.
///
/// Built with the folders keyed the way `finish_git_probe` keys them, and
/// with the nested layout, so this is the shape a real session produces.
fn nested_worktree_snapshot(seq: u64, focused: Option<u32>) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(1)),
        name: wire::AGENT_SNAPSHOT.to_owned(),
        payload: Some(
            serde_json::to_string(&PollReply {
                suppressed_panes: Vec::new(),
                panes: Some(vec![
                    agent_in(3, "/work/hocus", TabId::new(1)),
                    agent_in(5, NESTED_WORKTREE, TabId::new(2)),
                ]),
                activity: agents::Activity::new(),
                manual_kinds: BTreeMap::new(),
                yinz_session_name: "test-session".to_owned().into(),
                tab_projects: BTreeMap::new(),
                repositories: BTreeMap::from([
                    (
                        Location::local("/work/hocus"),
                        on_branch("/work/hocus", "main"),
                    ),
                    (
                        Location::local(NESTED_WORKTREE),
                        worktree_of(NESTED_WORKTREE, "/work/hocus", "topic"),
                    ),
                ]),
                focused,
                summaries: agents::Summaries::new(),
                hooks: agents::HookFeeds::new(),
                runs: Vec::new(),
            })
            .expect("serialize"),
        ),
        args: BTreeMap::from([("seq".to_owned(), seq.to_string())]),
        is_private: true,
        pane_id: None,
    }
}

// -----------------------------------------------------------------------
// Questions over the `hocus_ask` pipe
// -----------------------------------------------------------------------

/// As a plugin asker sends one: a plugin-sourced pipe carrying a `wire::Ask`.
fn ask(text: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(7)),
        name: wire::ASK.to_owned(),
        payload: Some(
            serde_json::to_string(&wire::Ask {
                text: text.to_owned(),
            })
            .expect("encoded"),
        ),
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    }
}

fn user_messages(state: &State) -> Vec<&str> {
    state
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .map(|message| message.content.as_str())
        .collect()
}

mod agent_history_tools;
mod agent_snapshots;
mod agent_tools;
mod auto_approval;
mod chat_tabs;
mod chats;
mod commands;
mod confirmation;
mod data_store;
mod decisions;
mod extensions;
mod file_tools;
mod jev_calibration;
mod jobs;
mod keys;
mod note_tools;
mod picker;
mod plan_mode;
mod plugin_tools;
mod project_assignment;
mod project_discovery;
mod questions;
mod queueing;
mod quit_notice;
mod repository_tools;
mod skill_tools;
mod slash_commands;
mod streaming;
mod strip;
mod swamp;
mod todo_tools;
mod token_usage;
mod tool_families;
mod tool_rounds;
mod viewport;
mod vm_tools;
mod web_search;

/// Placement, or anything else outside hocus, can rename our pane after we last
/// asserted a title. Forgetting what we asked for is what makes the next render
/// say it again — the memo that keeps a render from restating an unchanged title
/// must not also let a stale one stand. See `State::last_pane_title`.
#[test]
fn a_pane_renamed_from_outside_is_titled_again_on_the_next_render() {
    let mut state = idle_state();
    state.own_plugin_id = yinz_tile::prelude::PluginId::new(7);
    state.last_pane_title = Some("hocus — a-model".to_owned());

    let mut own = pane_info(7, "hocus — a-model", None);
    own.id = yinz_tile::prelude::PaneId::from_legacy(own.id.legacy_parts().0, true);
    state.on_panes(manifest(vec![own.clone()]));
    assert_eq!(
        state.last_pane_title.as_deref(),
        Some("hocus — a-model"),
        "the pane says what we asked for, so there is nothing to redo"
    );

    let mut renamed = pane_info(7, "something else", None);
    renamed.id = yinz_tile::prelude::PaneId::from_legacy(renamed.id.legacy_parts().0, true);
    state.on_panes(manifest(vec![renamed]));
    assert_eq!(
        state.last_pane_title, None,
        "someone else renamed it, so the next render asserts the title again"
    );
}

mod workflow_tools;

fn add_background_store(state: &mut State) {
    state.plugin_instances.insert(
        yinz_tile::prelude::PluginId::new(90),
        PluginInstance {
            has_pane: false,
            location: "yinz:hocus-store".into(),
            configuration: BTreeMap::new(),
        },
    );
}

/// The service owns the write; chat changes only on the watch and correlated reply.
fn finish_background_project_change(state: &mut State) {
    assert!(matches!(state.status, Status::AwaitingExtension));
    let pending = state.workflow_call.clone().unwrap();
    let call = state.pending.front().cloned().unwrap();
    let args = tools::Args::parse(&call.arguments).unwrap();
    let rows = state
        .store()
        .fetch(&FetchRequest::entity("project"))
        .unwrap();
    let mutations = match call.name.as_str() {
        "project_rename" => {
            projects::plan_rename(
                &rows,
                args.name.as_deref().unwrap(),
                args.new_name.as_deref().unwrap(),
            )
            .unwrap()
            .mutations
        },
        "project_merge" => {
            projects::plan_merge(
                &rows,
                args.source.as_deref().unwrap(),
                args.target.as_deref().unwrap(),
            )
            .unwrap()
            .mutations
        },
        "project_forget" => vec![projects::delete_mutation(
            args.name.as_deref().unwrap().trim(),
        )],
        _ => panic!("not a project change"),
    };
    state.store().apply(mutations).unwrap();
    let rows = state
        .store()
        .fetch(&FetchRequest::entity("project"))
        .unwrap();
    state.take_projects(projects::from_rows(&rows), projects::row_ids(&rows));
    let reply = serde_json::json!({"version":1,"call_id":pending.rpc_id,"result":{"ok":true,"data":call.name}}).to_string();
    assert!(
        !state.on_workflow_reply(yinz_tile::prelude::PluginId::new(91), &reply, false),
        "another plugin cannot complete the request"
    );
    assert!(state.on_workflow_reply(pending.owner.unwrap(), &reply, false));
    assert!(
        !state.on_workflow_reply(pending.owner.unwrap(), &reply, false),
        "a duplicate response cannot execute twice"
    );
}
