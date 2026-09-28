use super::*;
use yinz_tile::prelude::DataValue;

#[test]
fn conversation_focus_enters_the_host_panel_bar() {
    let mut state = State::default();
    state.take_ring_stop(Some(hocus_proto::vms::STOP_TABS));
    assert!(state.tabs_focused);
    assert_eq!(state.bar_focused, [state.own_client_id]);
    state.take_ring_stop(Some(wire::STOP_LIST));
    assert!(!state.tabs_focused);
    assert!(state.bar_focused.is_empty());
}

fn item(id: u64, text: &str, done: bool, children: Vec<todos::Item>) -> todos::Item {
    todos::Item {
        id,
        text: text.into(),
        done,
        rev: DataRevision::new(id as i64),
        children,
    }
}

fn key(ch: char) -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(ch))
}

fn bare(key: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(key)
}

fn typed(state: &mut State, text: &str) {
    for ch in text.chars() {
        state.todo_key(key(ch));
    }
}

fn draft(state: &State) -> Option<&str> {
    match &state.todos.mode {
        Mode::Input { input, .. } => Some(input.get_text()),
        _ => None,
    }
}

fn refusal(state: &State) -> Option<&str> {
    match &state.todos.mode {
        Mode::Input { refusal, .. } => refusal.as_deref(),
        _ => None,
    }
}

fn project(name: &str, root: &str) -> projects::Project {
    projects::Project {
        name: name.into(),
        root: root.into(),
        firecracker_image: None,
        worktree_dir: None,
        remote: None,
        worktree_status: None,
        common_dir: None,
        discovered: false,
        replaced_ids: Vec::new(),
    }
}

/// A panel over a real store, open on /work and holding what `seed` put there.
fn stored(seed: impl FnOnce(&store::Local)) -> (store::Local, State) {
    let backend = store::Local::shared("hocus-agents");
    backend.define_all(&store::shared_defs()).unwrap();
    seed(&backend);
    let mut state = State {
        ..Default::default()
    };
    state.watch_todo_root_with(&backend, Some("/work".into()));
    (backend, state)
}

/// A todo written straight to the store, as another session would: this pane hears
/// nothing of it until it reads the list again.
fn put(backend: &impl Backend, text: &str, parent: Option<u64>) -> u64 {
    let mut put = Mutation::put("todo")
        .set("root", "/work")
        .set("text", text)
        .set("done", false);
    if let Some(parent) = parent {
        put = put.set("parent_id", parent);
    }
    backend.apply(vec![put.build()]).unwrap().ids[0].get() as u64
}

/// Another session rewording a todo behind this pane's back.
fn retext(backend: &impl Backend, id: u64, text: &str) {
    backend
        .apply(vec![Mutation::update("todo", Pred::eq("id", id))
            .set("text", text)
            .build()])
        .unwrap();
}

fn texts(state: &State) -> Vec<String> {
    todo_rows(&state.todos.list, true)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

#[test]
fn starting_a_todo_prefills_its_project_and_open_work_without_changing_the_list() {
    let mut state = State::default();
    state.todos.initial_cwd = "/somewhere/else".into();
    state.todos.project_root = Some("/work".into());
    state.todos.list.items = vec![item(
        1,
        "Improve the build",
        false,
        vec![
            item(
                2,
                "Handle `quotes` and $(commands)\nKeep each argument intact",
                false,
                vec![
                    item(3, "Already finished", true, vec![]),
                    item(
                        4,
                        "Cover multiline prompts",
                        false,
                        vec![item(5, "Check newlines", false, vec![])],
                    ),
                ],
            ),
            item(6, "Unrelated sibling", false, vec![]),
        ],
    )];
    state.todos.selected = Some(2);
    let before = state.todos.list.clone();
    let configuration = state.todo_agent_configuration().unwrap();
    assert_eq!(configuration["caller_cwd"], "/work");
    assert_eq!(configuration["overlay"], "true");
    assert_eq!(configuration["modal"], "true");
    assert_eq!(
        configuration["initial_prompt"],
        "Todo #2: Handle `quotes` and $(commands)\nKeep each argument intact\n\nParent context (for reference):\n- Improve the build\n\nOpen subtasks:\n- [ ] Cover multiline prompts\n  - [ ] Check newlines\n\nImplement the task, run relevant checks, and summarize the changes and anything still outstanding.",
    );
    assert!(state.todo_key(key('n')));
    assert_eq!(state.todos.list, before);
    // The panel stays up under the launcher, so that its Esc hands the keys back here.
    assert_eq!(state.panel_hides, 0);
    state.todos.selected = None;
    assert!(!state.todo_key(key('n')));
    state.todos.selected = Some(999);
    assert!(state.todo_agent_configuration().is_none());
}

#[test]
fn a_truncated_watch_loads_the_complete_list_and_keeps_it_on_fetch_failure() {
    let backend = store::Local::shared("hocus-agents");
    backend.define_all(&store::shared_defs()).unwrap();
    backend
        .apply(
            (0..1200)
                .map(|id| {
                    Mutation::put("todo")
                        .set("root", "/work")
                        .set("text", format!("item {id}"))
                        .set("done", true)
                        .build()
                })
                .collect(),
        )
        .unwrap();
    let rows = backend
        .fetch(&FetchRequest::entity("todo").limit(store::LIST_FETCH_LIMIT))
        .unwrap();
    assert!(rows.truncated);
    let mut state = State::default();
    state.todos.project_root = Some("/work".into());
    state.take_todo_rows(&backend, &rows);
    assert_eq!(todos::counts(&state.todos.list), (0, 1200));
    let tail = state.todos.list.items.last().unwrap().id;
    let edit = todos::plan_edit(&state.todos.list, tail, "edited", None)
        .unwrap()
        .0;
    assert!(matches!(
        state.apply_todo_plan(&backend, Ok(edit)),
        Applied::Saved(_)
    ));
    assert_eq!(
        todos::item_of(&state.todos.list, tail).unwrap().text,
        "edited"
    );
    let delete = todos::plan_delete(&state.todos.list, "/work", tail, false)
        .unwrap()
        .0;
    state.apply_todo_plan(&backend, Ok(delete));
    assert!(todos::item_of(&state.todos.list, tail).is_none());
    let add = todos::plan_add(&state.todos.list, "/work", "new", None)
        .unwrap()
        .0;
    state.apply_todo_plan(&backend, Ok(add));
    assert_eq!(todos::counts(&state.todos.list), (1, 1200));
    let before = state.todos.list.clone();
    state.take_todo_rows(&Host::shared(), &rows);
    assert!(!state.todos.status.is_empty());
    assert_eq!(state.todos.list, before);
    // A fetch that works again is past it.
    state.take_todo_rows(&backend, &rows);
    assert!(state.todos.status.is_empty());
}

#[test]
fn completed_subtrees_are_hidden_and_move_targets_exclude_the_source_subtree() {
    let list = todos::List {
        items: vec![
            item(1, "parent", false, vec![item(2, "child", false, vec![])]),
            item(3, "done", true, vec![item(4, "done child", true, vec![])]),
            item(5, "other", false, vec![]),
        ],
        ..Default::default()
    };
    assert_eq!(
        todo_rows(&list, false)
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 2, 5]
    );
    let mut state = State::default();
    state.todos.list = list;
    assert_eq!(state.move_targets(1), [5]);
    state.todos.show_done = true;
    assert_eq!(state.move_targets(1), [3, 4, 5]);
}

fn two_todos() -> State {
    let mut state = State {
        ..Default::default()
    };
    state.todos.project_root = Some("/work".into());
    state.todos.list.items = vec![item(1, "one", false, vec![]), item(2, "two", false, vec![])];
    state.todos.selected = Some(1);
    state
}

/// Leaving the tab drops a delete or complete question armed there, as a VMs question is
/// dropped: left armed, it answered the first key of the next visit, and a `y` then deleted
/// or completed what was asked about a list the user had long stopped looking at.
#[test]
fn a_tab_switch_drops_an_armed_todo_question() {
    let mut state = two_todos();
    assert!(state.todo_key(key('d')));
    assert!(matches!(state.todos.mode, Mode::Delete { .. }));
    state.update(Event::Visible(false));
    state.update(Event::Visible(true));
    assert!(matches!(state.todos.mode, Mode::Normal));
}

/// A tab click, like the bar it no longer focuses, hands the keys to the list: `j` walks
/// it. List keys are consumed while the bar has focus.
#[test]
fn queued_bar_keys_are_consumed_until_the_bar_releases_focus() {
    let mut state = two_todos();
    state.render(12, 40);
    state.tabs_focused = false;
    assert!(!state.tabs_focused);
    assert!(state.on_key(key('j')));
    assert_eq!(state.todos.selected, Some(2));

    state.on_key(bare(BareKey::Tab).with_shift_modifier());
    assert!(state.tabs_focused);
    assert!(state.on_key(key('a')));
    assert!(state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.on_key(bare(BareKey::Esc)));
    assert!(state.on_key(key('a')));
    assert!(matches!(state.todos.mode, Mode::Input { .. }));

    // Ctrl-o is the panel's, taken before the box that answers every other key.
    state.on_key(key('x'));
    assert!(!state.on_key(key('o').with_ctrl_modifier()));
    assert_eq!(draft(&state), Some("x"));
}

/// While the tab bar has the keys the divider says what they do there, in place of the
/// list's keys - on the row the list already gives it, not on one more - and a note or a
/// problem still leads it.
#[test]
fn the_bar_hint_takes_the_divider_not_a_row() {
    let mut state = two_todos();
    state.render_todos(12, 60);
    let rows = state.todos.last_list_rows;
    state.tabs_focused = true;
    state.render_todos(12, 60);
    assert_eq!(state.todos.last_list_rows, rows);
    let line = state.todo_divider(60, 0, 0).line;
    assert!(line.contains("←/→ choose"), "{line}");
    // Enter on the bar steps into the list rather than editing.
    assert!(!line.contains("Enter/e edit"), "{line}");
    state.todos.status = "boom".into();
    let line = state.todo_divider(80, 0, 0).line;
    assert!(line.contains("boom · Ctrl-r retry · ←/→ choose"), "{line}");
    state.todos.status.clear();
    // Nor while another pane has the keyboard: the bar does not have it either.
    state.has_keyboard = Some(false);
    let line = state.todo_divider(60, 0, 0).line;
    assert!(
        line.contains("Space done") && !line.contains("←/→"),
        "{line}"
    );
    state.has_keyboard = None;
    assert!(state.on_key(bare(BareKey::Esc)));

    // A move and a project pick keep across the Shift+Tab up to the bar, whose Enter and
    // Esc step back into the list rather than nesting or cancelling: the divider names
    // the bar's keys over them too - the move still what it is moving - and the mode's
    // own keys once the keys are back.
    for (arm, kept) in [('m', "moving \"one\""), ('p', "")] {
        assert!(state.on_key(key(arm)));
        let own = state.todo_divider(80, 0, 0).line;
        assert!(
            own.contains("Esc cancel") || own.contains("Esc back"),
            "{own}"
        );
        assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(!matches!(state.todos.mode, Mode::Normal), "{arm:?}");
        let line = state.todo_divider(80, 0, 0).line;
        assert!(line.contains("←/→ choose") && line.contains(kept), "{line}");
        assert!(
            !line.contains("Esc cancel") && !line.contains("Esc back"),
            "{line}"
        );
        assert!(state.on_key(bare(BareKey::Esc)));
        assert!(!state.tabs_focused);
        assert_eq!(state.todo_divider(80, 0, 0).line, own);
        assert!(state.on_key(bare(BareKey::Esc)));
        assert!(matches!(state.todos.mode, Mode::Normal), "{arm:?}");
    }
}

/// A box kept open across the Shift+Tab up to the bar is drawn in the divider's place, so
/// its row is where the bar's keys go: the box's own would say Enter saves and Esc closes,
/// and on the bar both step back down into the box instead. The draft stays on the row,
/// the row under the list the draft pushes the keys onto names the bar's too, and the
/// box's keys come back with the keys.
#[test]
fn the_bar_hint_takes_an_open_boxs_row_too() {
    let mut state = two_todos();
    state.render_todos(12, 60);
    assert!(state.on_key(key('a')));
    for ch in "Fix".chars() {
        state.on_key(key(ch));
    }
    state.render_todos(12, 60);
    let rows = state.todos.last_list_rows;
    let own = state.input_divider(60).unwrap().0.line;
    assert!(own.contains("Fix") && own.contains("Enter save"), "{own}");

    assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Input { .. }));
    let line = state.input_divider(60).unwrap().0.line;
    assert!(
        line.contains("Fix") && line.contains("←/→ choose"),
        "{line}"
    );
    let spilled = state.input_hint_row(60).unwrap_or_default();
    let narrow = state.input_hint_row(24).unwrap_or_default();
    for text in [&line, &spilled, &narrow] {
        for gone in ["Enter save", "Esc keep draft", "Esc cancel", "Alt+Enter"] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
    }
    assert!(spilled.contains("←/→ choose"), "{spilled}");
    assert!(narrow.contains("←/→ choose"), "{narrow}");
    // Both sets of keys spill past a draft this long at this width, so the list keeps its
    // rows across the Shift+Tab.
    state.render_todos(12, 60);
    assert_eq!(state.todos.last_list_rows, rows);

    assert!(state.on_key(bare(BareKey::Esc)));
    assert!(!state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Input { .. }));
    assert_eq!(draft(&state), Some("Fix"));
    assert_eq!(state.input_divider(60).unwrap().0.line, own);
}

/// Putting the panel away drops a question or a pick left open. A box is put away as a
/// draft, which the key that opened it brings back - not left open to take the first
/// letters of the next visit.
#[test]
fn a_hide_drops_todo_questions_and_keeps_the_draft() {
    let mut state = two_todos();
    state.todos.list.items[0].children = vec![item(3, "three", false, vec![])];
    for arm in ['d', 'm', 'p', ' '] {
        state.on_key(key(arm));
        assert!(!matches!(state.todos.mode, Mode::Normal), "{arm:?}");
        state.hide_the_panel();
        assert!(matches!(state.todos.mode, Mode::Normal), "{arm:?}");
    }
    state.on_key(key('a'));
    state.on_key(key('x'));
    state.hide_the_panel();
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
    state.on_key(key('j'));
    assert!(matches!(state.todos.mode, Mode::Normal));
    state.on_key(key('a'));
    assert_eq!(draft(&state), Some("x"));
}

#[test]
fn todo_modes_capture_the_item_and_accept_shifted_text() {
    let mut state = State::default();
    state.todos.project_root = Some("/work".into());
    state.todos.list.items = vec![
        item(1, "parent", false, vec![item(2, "child", false, vec![])]),
        item(3, "other", false, vec![]),
    ];
    state.todos.selected = Some(1);

    assert!(state.todo_key(key('A')));
    assert!(matches!(
        state.todos.mode,
        Mode::Input {
            action: InputAction::Add { parent_id: Some(1) },
            ..
        }
    ));
    state.todo_key(key('X').with_shift_modifier());
    assert_eq!(draft(&state), Some("X"));
    state.todo_key(bare(BareKey::Esc));

    state.todo_key(key('d'));
    assert!(matches!(
        state.todos.mode,
        Mode::Delete {
            id: 1,
            descendants: 1,
            ..
        }
    ));
    state.todo_key(key('n'));
    assert!(matches!(state.todos.mode, Mode::Normal));

    state.todo_key(key('m'));
    assert!(matches!(
        state.todos.mode,
        Mode::Move {
            id: 1,
            rev,
            target: Some(3)
        } if rev == DataRevision::new(1)
    ));
}

/// Rows as a watch delivers them: ids `1..=count` under /work, each one under 1 but 1.
fn watched(count: i64) -> DataRows {
    DataRows {
        columns: ["id", "root", "text", "parent_id", "rev"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        rows: (1..=count)
            .map(|id| {
                vec![
                    DataValue::Int(id),
                    DataValue::Text("/work".into()),
                    DataValue::Text(format!("todo {id}")),
                    if id == 1 {
                        DataValue::Null
                    } else {
                        DataValue::Int(1)
                    },
                    DataValue::Int(id),
                ]
            })
            .collect(),
        truncated: false,
        next_cursor: None,
    }
}

fn parent_and_child() -> State {
    let mut state = State {
        ..Default::default()
    };
    state.todos.project_root = Some("/work".into());
    state.todos.todo_watch = Some(DataWatchId::new(1));
    state.todos.list.items = vec![item(
        1,
        "parent",
        false,
        vec![item(2, "child", false, vec![])],
    )];
    state.todos.selected = Some(1);
    state
}

#[test]
fn delete_confirmation_keeps_its_plan_across_watch_updates() {
    let mut state = parent_and_child();
    let expected = todos::plan_delete(&state.todos.list, "/work", 1, true)
        .unwrap()
        .0;
    state.todo_key(key('d'));
    assert!(state.todo_rows_changed(DataWatchId::new(1), watched(3)));
    assert_eq!(state.todos.list.items[0].children.len(), 2);
    let Mode::Delete {
        plan, descendants, ..
    } = &state.todos.mode
    else {
        panic!("confirmation was discarded");
    };
    assert_eq!(*descendants, 1);
    assert_eq!(*plan, expected);
    assert_ne!(
        *plan,
        todos::plan_delete(&state.todos.list, "/work", 1, true)
            .unwrap()
            .0
    );
}

/// The question answered is the one that was asked: a subtask arriving while it is up
/// does not join what `y` completes.
#[test]
fn complete_confirmation_keeps_its_plan_across_watch_updates() {
    let mut state = parent_and_child();
    let expected = todos::plan_set_done(&state.todos.list, "/work", 1, true)
        .unwrap()
        .0;
    state.todo_key(key(' '));
    assert!(state.todo_rows_changed(DataWatchId::new(1), watched(3)));
    let Mode::Complete { plan, cascaded, .. } = &state.todos.mode else {
        panic!("confirmation was discarded");
    };
    assert_eq!(*cascaded, 1);
    assert_eq!(*plan, expected);
    // The item it was about going is another matter: nothing is left to ask about.
    assert!(state.todo_rows_changed(
        DataWatchId::new(1),
        DataRows {
            rows: Vec::new(),
            ..watched(0)
        }
    ));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);
}

/// Enter opens the selected item, as it does on every other tab - into the box, here -
/// and never completes it.
#[test]
fn enter_opens_the_todo_for_editing_instead_of_completing_it() {
    let mut state = two_todos();
    assert!(state.todo_key(bare(BareKey::Enter)));
    assert!(matches!(
        &state.todos.mode,
        Mode::Input {
            action: InputAction::Edit { id: 1, .. },
            ..
        }
    ));
    assert_eq!(draft(&state), Some("one"));
    assert!(!state.todos.list.items[0].done);
    assert!(state.todos.note.is_none());
    assert!(state.todo_divider(80, 0, 0).line.contains("Enter/e edit"));
}

/// Space on a todo with open work under it asks first, leading with the answer and the
/// count so a narrow pane still shows both; anything but `y` writes nothing.
#[test]
fn completing_a_parent_with_open_subtasks_asks_first() {
    let (backend, mut state) = stored(|backend| {
        let parent = put(backend, "Fix resize", None);
        put(backend, "open child", Some(parent));
        let done = put(backend, "done child", Some(parent));
        backend
            .apply(vec![Mutation::update("todo", Pred::eq("id", done))
                .set("done", true)
                .build()])
            .unwrap();
    });
    let parent = state.todos.selected.unwrap();
    assert!(state.todo_key_with(&backend, key(' ')));
    assert!(matches!(
        state.todos.mode,
        Mode::Complete { id, cascaded: 1, .. } if id == parent
    ));
    let question = state.todo_divider(24, 5, 5);
    assert_eq!(question.line.width(), 24);
    assert!(
        question.line.contains("y/n complete with 1"),
        "{}",
        question.line
    );
    assert!(question.error);

    assert!(state.todo_key_with(&backend, key('n')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.todos.note.is_none());
    assert_eq!(todos::counts(&state.todos.list), (2, 3));

    state.todo_key_with(&backend, key(' '));
    assert!(state.todo_key_with(&backend, key('y')));
    assert_eq!(todos::counts(&state.todos.list), (0, 3));
    let note = state.todos.note.as_ref().unwrap();
    assert_eq!(note.text(60), "completed \"Fix resize\" +1 subtask");
    assert_eq!(note.hint, "v shows done");
}

#[test]
fn completing_a_leaf_needs_no_confirmation() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "leaf", None);
    });
    let leaf = state.todos.selected.unwrap();
    assert!(state.todo_key_with(&backend, key(' ')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(todos::item_of(&state.todos.list, leaf).unwrap().done);
    assert_eq!(
        state.todos.note.as_ref().unwrap().text(60),
        "completed \"leaf\""
    );
}

/// A subtree's delete question keeps its `y/n` and its size on the default panel's
/// narrowest half, with no scroll readout crowding it.
#[test]
fn a_delete_question_leads_with_its_answer_and_its_size() {
    let mut state = parent_and_child();
    state.todos.list.items[0].children[0].children = vec![item(3, "grandchild", true, vec![])];
    state.todo_key(key('d'));
    let question = state.todo_divider(24, 7, 3);
    assert!(
        question.line.starts_with("─ y/n delete with 2"),
        "{}",
        question.line
    );
    assert!(!question.line.contains('↑'));
    assert!(question.error);
    state.todo_key(key('n'));
    state.todos.selected = Some(2);
    state.todos.list.items[0].children[0].children.clear();
    state.todo_key(key('d'));
    assert!(state
        .todo_divider(24, 0, 0)
        .line
        .contains("y/n delete this todo"));
}

#[test]
fn delete_planning_errors_do_not_open_confirmation() {
    let mut state = State::default();
    state.todos.project_root = Some("/work".into());
    state.todos.selected = Some(1);
    assert!(state.todo_key(key('d')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    // In the panel's words, not the model's `call todo_list`.
    assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);
}

/// A paste is typing: it goes in at the caret, and a pasted list stays a list, drawn on
/// the box's one row with a `⏎` for each break rather than a break that scrolls the pane.
#[test]
fn a_paste_lands_at_the_caret_and_keeps_its_line_breaks() {
    let (backend, mut state) = stored(|_| {});
    assert!(!state.todo_paste("ignored"));
    state.todo_key_with(&backend, key('a'));
    typed(&mut state, "Fix ");
    assert!(state.todo_paste("flaky\r\nresize\ttest\n"));
    assert_eq!(draft(&state), Some("Fix flaky\nresize test"));
    let (row, _) = state.input_divider(60).unwrap();
    assert!(row.line.contains("Fix flaky⏎resize test"), "{}", row.line);
    assert!(!row.line.chars().any(char::is_control));
    state.todo_key_with(&backend, bare(BareKey::Home));
    assert!(state.todo_paste("Now: "));
    assert_eq!(draft(&state), Some("Now: Fix flaky\nresize test"));
    assert!(state.todo_key_with(&backend, bare(BareKey::Enter)));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(texts(&state), ["Now: Fix flaky\nresize test"]);
}

/// Whatever the width, the box fits it, the caret is on it and the end of a draft being
/// typed is what shows - and the keys follow the draft only once all of it fits.
#[test]
fn the_input_label_fits_every_width_and_keeps_the_caret_on_screen() {
    let draft: String = (0..60).map(|at| char::from(b'a' + at % 26)).collect();
    for room in 1..=80 {
        let (label, caret, text, appended) =
            input_label("new: ", &draft, 60, &["Enter save", "Esc keep draft"], room);
        assert!(label.width() <= room, "{room}: {label}");
        assert!(caret < room, "{room}: {label}");
        if room > "new: ".width() + 1 {
            let shown: String = label.chars().skip(text.start).take(text.len()).collect();
            assert!(
                !shown.is_empty() && draft.ends_with(&shown),
                "{room}: {label}"
            );
        }
        assert_eq!(label.contains("Enter save"), room >= 78, "{room}: {label}");
        assert_eq!(appended, label.matches(" · ").count(), "{room}: {label}");
    }
}

#[test]
fn the_box_shows_the_end_of_a_long_draft_with_the_caret_after_it() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "Investigate flaky resize integration test");
    state.render_todos(12, 24);
    let (x, y) = state.todo_input_cursor(12, 24).unwrap();
    assert_eq!(y, 12, "the divider, the pane's last row under the tab bar");
    let (row, caret) = state.input_divider(24).unwrap();
    assert_eq!(x, caret);
    assert_eq!(row.line.width(), 24);
    let before: String = row
        .line
        .chars()
        .scan(0, |width, ch| {
            *width += ch.to_string().width();
            (*width <= caret).then_some(ch)
        })
        .collect();
    assert!(before.starts_with("─ new: "), "{}", row.line);
    assert!(before.ends_with("test"), "{}", row.line);
    // The typed text is the bright part; the prompt and the rule are dim.
    let bright = row.bright.unwrap();
    let text: String = row
        .line
        .chars()
        .skip(bright.start)
        .take(bright.len())
        .collect();
    assert!(text.ends_with("test") && !text.contains("new"), "{text}");
    assert_eq!(state.todo_input_cursor(12, 3), None);
    state.todo_key(bare(BareKey::Esc));
    assert_eq!(state.todo_input_cursor(12, 24), None);
}

/// A draft too long for the box's row to carry its keys gets a row for them above it, from
/// the list's, and gives it back once they fit again; a refusal says what to press instead.
#[test]
fn the_box_keys_get_a_row_of_their_own_when_the_draft_leaves_no_room() {
    let mut state = two_todos();
    state.render_todos(12, 24);
    let normal = state.todos.last_list_rows;
    state.todo_key(key('a'));
    typed(&mut state, "Fix");
    state.render_todos(12, 24);
    let keys = state.input_hint_row(24).unwrap();
    assert!(keys.contains("Enter save"), "{keys}");
    assert!(!keys.contains("? keys"), "{keys}");
    assert_eq!(state.todos.last_list_rows, normal - 1);
    // The caret stays on the box, the pane's last row under the tab bar.
    assert_eq!(state.todo_input_cursor(12, 24).map(|(_, y)| y), Some(12));
    // `Enter save · Esc keep draft` is 27 columns: at 24 the row holds the first alone.
    let keys = state.input_hint_row(28).unwrap();
    assert!(
        keys.contains("Enter save") && keys.contains("Esc keep draft"),
        "{keys}"
    );

    state.render_todos(12, 80);
    assert_eq!(state.input_hint_row(80), None);
    assert_eq!(state.todos.last_list_rows, normal);

    state.refuse_todo_input("the list changed · Enter tries again".into());
    state.render_todos(12, 24);
    assert_eq!(
        state.todos.last_list_rows,
        normal - 2,
        "the refusal's two rows"
    );
}

#[test]
fn edit_moves_the_caret_to_fix_a_mid_text_typo() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "Fix teh resize bug", None);
    });
    state.todo_key_with(&backend, key('e'));
    state.todo_key_with(&backend, bare(BareKey::Home));
    for _ in 0..7 {
        state.todo_key_with(&backend, bare(BareKey::Right));
    }
    for _ in 0..2 {
        state.todo_key_with(&backend, bare(BareKey::Backspace));
    }
    typed(&mut state, "he");
    assert_eq!(draft(&state), Some("Fix the resize bug"));
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(texts(&state), ["Fix the resize bug"]);
    assert_eq!(
        state.todos.note.as_ref().unwrap().text(60),
        "saved \"Fix the resize bug\""
    );
}

#[test]
fn word_keys_edit_by_the_word_and_alt_letters_type_nothing() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "one two three");
    state.todo_key(key('w').with_ctrl_modifier());
    assert_eq!(draft(&state), Some("one two "));
    state.todo_key(bare(BareKey::Backspace).with_alt_modifier());
    assert_eq!(draft(&state), Some("one "));
    state.todo_key(key('x').with_alt_modifier());
    assert_eq!(draft(&state), Some("one "));
    state.todo_key(key('u').with_ctrl_modifier());
    assert_eq!(draft(&state), Some(""));
}

/// Alt+Enter and Shift+Enter are the conversation's keys for a line break, and here too;
/// only a bare Enter saves. The break shows as `⏎`, never as a raw line feed.
#[test]
fn alt_enter_starts_a_new_line_shown_as_a_return_symbol() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "a\nb", None);
    });
    state.todo_key_with(&backend, key('e'));
    state.todo_key_with(&backend, bare(BareKey::Backspace));
    assert_eq!(draft(&state), Some("a\n"));
    state.todo_key_with(&backend, bare(BareKey::Enter).with_alt_modifier());
    state.todo_key_with(&backend, bare(BareKey::Enter).with_shift_modifier());
    typed(&mut state, "c");
    assert_eq!(draft(&state), Some("a\n\n\nc"));
    let (row, _) = state.input_divider(40).unwrap();
    assert!(row.line.contains("a⏎⏎⏎c"), "{}", row.line);
    assert!(!row.line.chars().any(char::is_control));
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(texts(&state), ["a\n\n\nc"]);
}

/// An add guards the whole project, so another session touching any item refuses it. One
/// that left the add's own parent alone is no reason to stop: it is planned again against
/// the fresh list and saved, and the new todo is selected.
#[test]
fn an_add_survives_an_unrelated_edit_from_another_writer() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "one", None);
        put(backend, "two", None);
    });
    let two = state.todos.list.items[1].id;
    retext(&backend, two, "two, edited elsewhere");
    state.todo_key_with(&backend, key('a'));
    typed(&mut state, "three");
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(
        matches!(state.todos.mode, Mode::Normal),
        "{:?}",
        refusal(&state)
    );
    assert_eq!(texts(&state), ["one", "two, edited elsewhere", "three"]);
    let three = state.todos.list.items[2].id;
    assert_eq!(state.todos.selected, Some(three));
    assert_eq!(
        state.todos.note.as_ref().unwrap().text(60),
        "added \"three\""
    );
}

/// A change to what the add hangs from is a fresh decision for the user: the list is
/// reloaded under the box, which keeps its text, and the next Enter saves against it.
#[test]
fn a_conflicting_add_reloads_the_list_and_keeps_the_typed_text() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "parent", None);
    });
    let parent = state.todos.selected.unwrap();
    state.todo_key_with(&backend, key('A'));
    typed(&mut state, "child");
    retext(&backend, parent, "parent, renamed elsewhere");
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert_eq!(draft(&state), Some("child"));
    assert!(refusal(&state).unwrap().contains("changed elsewhere"));
    assert_eq!(texts(&state), ["parent, renamed elsewhere"]);
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(texts(&state), ["parent, renamed elsewhere", "child"]);
}

/// An edit is never taken over quietly: the other writer's text goes on screen, Enter
/// overwrites it because the user said so, and Esc keeps theirs - and the user's words as
/// a draft.
#[test]
fn an_edit_conflict_asks_before_overwriting() {
    for overwrite in [true, false] {
        let (backend, mut state) = stored(|backend| {
            put(backend, "mine", None);
        });
        let id = state.todos.selected.unwrap();
        state.todo_key_with(&backend, key('e'));
        typed(&mut state, " and more");
        retext(&backend, id, "theirs");
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert_eq!(draft(&state), Some("mine and more"));
        assert!(refusal(&state).unwrap().contains("Enter overwrites"));
        assert!(refusal(&state).unwrap().contains("yours stays a draft"));
        assert_eq!(texts(&state), ["theirs"]);
        // Over the box, wrapped rather than cut, so a narrow pane still names both keys -
        // and gives up the words about the draft, which the box shows anyway, first.
        let segments: Vec<&str> = refusal(&state).unwrap().split(" · ").collect();
        assert_eq!(
            wrap_segments(&segments, 24, 3),
            ["changed elsewhere", "Enter overwrites", "Esc keeps theirs"]
        );
        let wide = wrap_segments(&segments, 40, 3);
        assert_eq!(wide.len(), 2, "{wide:?}");
        assert!(wide[1].ends_with("yours stays a draft"), "{wide:?}");
        state.render_todos(12, 24);
        assert_eq!(state.todos.last_list_rows, 12 - 1 - 3 - 1);
        if overwrite {
            state.todo_key_with(&backend, bare(BareKey::Enter));
            assert!(matches!(state.todos.mode, Mode::Normal));
            assert_eq!(texts(&state), ["mine and more"]);
        } else {
            state.todo_key_with(&backend, bare(BareKey::Esc));
            assert_eq!(texts(&state), ["theirs"]);
            // Enter opens the item into the box as `e` does, draft and all.
            assert!(state
                .todo_divider(80, 0, 0)
                .line
                .contains("Enter/e resume draft"));
            state.todo_key_with(&backend, key('e'));
            assert_eq!(draft(&state), Some("mine and more"));
        }
    }
}

/// An overwrite a conflict armed, and then refused for another reason, is disarmed: the
/// refusal now drawn does not offer it, and Enter used to wait for `Enter overwrites` to be
/// drawn again - which it never was - doing nothing for good. The next Enter meets the
/// conflict afresh, and the overwrite it offers then goes through.
#[test]
fn an_overwrite_refused_for_another_reason_does_not_stall_enter() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "mine", None);
    });
    let id = state.todos.selected.unwrap();
    let set_text = |state: &mut State, text: &str| {
        if let Mode::Input { input, .. } = &mut state.todos.mode {
            *input = TextInput::new(text.to_owned());
        }
    };
    state.todo_key_with(&backend, key('e'));
    typed(&mut state, " and more");
    retext(&backend, id, "theirs");
    state.todo_key_with(&backend, bare(BareKey::Enter));
    state.render_todos(12, 40);
    assert!(state.todos.overwrite_shown);
    // Nothing visible to save: refused by the planner, not by a conflict.
    set_text(&mut state, "\u{7}");
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(refusal(&state).unwrap().contains("visible characters"));
    set_text(&mut state, "mine and more");
    state.render_todos(12, 40);
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(
        refusal(&state).unwrap().contains("Enter overwrites"),
        "asked again"
    );
    state.render_todos(12, 40);
    state.todo_key_with(&backend, bare(BareKey::Enter));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(texts(&state), ["mine and more"]);
}

/// On a pane too short for all of an edit conflict's refusal the project's row gives way
/// to it first, and an Enter whose `Enter overwrites` was never on screen overwrites
/// nothing.
#[test]
fn an_edit_conflict_on_a_short_pane_never_overwrites_unseen() {
    for (rows, cols) in [(3, 24), (4, 24), (5, 24), (6, 24), (3, 40), (2, 40)] {
        let (backend, mut state) = stored(|backend| {
            put(backend, "mine", None);
        });
        let id = state.todos.selected.unwrap();
        state.todo_key_with(&backend, key('e'));
        typed(&mut state, " and more");
        retext(&backend, id, "theirs");
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert!(refusal(&state).is_some(), "{rows}x{cols}");
        state.render_todos(rows, cols);
        match (rows, cols) {
            (5, 24) => assert_eq!(state.todos.last_header_rows, 0),
            (6, 24) => assert_eq!(state.todos.last_header_rows, 1),
            _ => {},
        }
        let shown = !(rows == 2 || (rows == 3 && cols == 24));
        assert_eq!(state.todos.overwrite_shown, shown, "{rows}x{cols}");
        state.todo_key_with(&backend, bare(BareKey::Enter));
        if shown {
            assert!(matches!(state.todos.mode, Mode::Normal), "{rows}x{cols}");
            assert_eq!(texts(&state), ["mine and more"], "{rows}x{cols}");
        } else {
            assert_eq!(draft(&state), Some("mine and more"), "{rows}x{cols}");
            assert_eq!(texts(&state), ["theirs"], "{rows}x{cols}");
        }
    }
}

#[test]
fn esc_keeps_a_draft_that_a_restores() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "half");
    state.todo_key(bare(BareKey::Esc));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
    // An edit opened and put away unchanged is not a draft, and does not replace one.
    state.todo_key(key('e'));
    assert_eq!(draft(&state), Some("one"));
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('A'));
    assert_eq!(draft(&state), Some(""), "a subtask is another entry");
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('a'));
    assert_eq!(draft(&state), Some("half"));
    // Saving it spends it.
    state.todo_key(bare(BareKey::Esc));
    assert!(!state.todos.drafts.is_empty());
    state.todo_key(key('a'));
    assert!(state.todos.drafts.is_empty());
}

/// A box still holding the text it opened with is no draft, even once the item has been
/// reworded elsewhere since: Esc puts it away with nothing kept, and `e` opens the new text.
#[test]
fn an_untouched_edit_is_no_draft_after_the_item_changes_elsewhere() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "one", None);
    });
    let id = state.todos.selected.unwrap();
    state.todo_key_with(&backend, key('e'));
    retext(&backend, id, "uno");
    state.reload_todos(&backend).unwrap();
    assert_eq!(draft(&state), Some("one"));
    let (row, _) = state.input_divider(80).unwrap();
    assert!(row.line.contains("Esc cancel"), "{}", row.line);
    state.todo_key_with(&backend, bare(BareKey::Esc));
    assert!(state.todos.drafts.is_empty());
    assert!(state.todo_divider(80, 0, 0).line.contains("Enter/e edit"));
    state.todo_key_with(&backend, key('e'));
    assert_eq!(draft(&state), Some("uno"));
}

/// A draft is its project's: a look at another one neither shows it nor drops it, and it
/// is back when its project is - as a new todo, if its item went meanwhile.
#[test]
fn a_draft_kept_by_esc_waits_for_its_project() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "one", None);
    });
    let one = state.todos.selected.unwrap();
    state.todo_key_with(&backend, key('a'));
    typed(&mut state, "half");
    state.todo_key_with(&backend, bare(BareKey::Esc));
    state.watch_todo_root_with(&backend, Some("/other".into()));
    assert!(!state.todo_divider(80, 0, 0).line.contains("resume draft"));
    state.todo_key_with(&backend, key('a'));
    assert_eq!(draft(&state), Some(""));
    state.todo_key_with(&backend, bare(BareKey::Esc));
    state.watch_todo_root_with(&backend, Some("/work".into()));
    assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
    state.todo_key_with(&backend, key('a'));
    assert_eq!(draft(&state), Some("half"));
    state.todo_key_with(&backend, bare(BareKey::Esc));

    state.todos.selected = Some(one);
    state.todo_key_with(&backend, key('e'));
    typed(&mut state, " more");
    state.todo_key_with(&backend, bare(BareKey::Esc));
    state.watch_todo_root_with(&backend, Some("/other".into()));
    backend
        .apply(vec![Mutation::delete("todo", Pred::eq("id", one))])
        .unwrap();
    state.watch_todo_root_with(&backend, Some("/work".into()));
    assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
    // Both new todos now, and neither lost: the edit's words did not take the place of the
    // draft kept before them. `a` brings them back one at a time.
    state.todo_key_with(&backend, key('a'));
    assert_eq!(draft(&state), Some("half"));
    state.todo_key_with(&backend, bare(BareKey::Esc));
    state.todo_key_with(&backend, key('a'));
    assert_eq!(draft(&state), Some("one more"));
}

/// A second box put away keeps its words beside the first box's: one slot for all of them
/// used to let an edit put away drop a new todo's text without a word.
#[test]
fn a_draft_for_one_entry_does_not_drop_another() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "buy milk");
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('e'));
    typed(&mut state, " and more");
    state.todo_key(bare(BareKey::Esc));
    let divider = state.todo_divider(80, 0, 0).line;
    assert!(divider.contains("Enter/e resume draft"), "{divider}");
    assert!(divider.contains("a resume draft"), "{divider}");
    state.todo_key(key('a'));
    assert_eq!(draft(&state), Some("buy milk"));
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('e'));
    assert!(draft(&state).is_some_and(|text| text.ends_with(" and more")));
}

/// An edit box nobody typed in holds only its item's text: with the item deleted elsewhere
/// it closes, rather than becoming a new todo that Esc would keep - in place of a draft the
/// user did type.
#[test]
fn an_untouched_edit_of_a_deleted_item_closes_without_a_draft() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "one", None);
    });
    let one = state.todos.selected.unwrap();
    state.todo_key_with(&backend, key('a'));
    typed(&mut state, "half");
    state.todo_key_with(&backend, bare(BareKey::Esc));
    state.todo_key_with(&backend, key('e'));
    assert_eq!(draft(&state), Some("one"));
    backend
        .apply(vec![Mutation::delete("todo", Pred::eq("id", one))])
        .unwrap();
    state.reload_todos(&backend).unwrap();
    assert!(matches!(state.todos.mode, Mode::Normal));
    state.todo_key_with(&backend, key('a'));
    assert_eq!(
        draft(&state),
        Some("half"),
        "the draft typed is the one kept"
    );
}

#[test]
fn blank_enter_closes_the_box_without_an_error() {
    let mut state = two_todos();
    for blank in ["", "   "] {
        state.todo_key(key('a'));
        typed(&mut state, blank);
        assert!(state.todo_key(bare(BareKey::Enter)));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todos.note.is_none());
        assert!(state.todos.drafts.is_empty());
    }
}

#[test]
fn add_is_refused_at_the_open_cap_before_the_box_opens() {
    let mut state = two_todos();
    state.todos.list.items = (1..=todos::MAX_OPEN as u64)
        .map(|id| item(id, "open", false, vec![]))
        .collect();
    assert!(state.todo_key(key('a')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    let note = state.todos.note.as_ref().unwrap();
    assert!(note.error && note.lead.contains("200"), "{}", note.lead);
}

#[test]
fn subtask_under_a_max_depth_parent_is_refused_before_typing() {
    let mut state = two_todos();
    let mut chain = item(todos::MAX_DEPTH as u64, "deepest", false, vec![]);
    for id in (1..todos::MAX_DEPTH as u64).rev() {
        chain = item(id, "level", false, vec![chain]);
    }
    state.todos.list.items = vec![chain];
    state.todos.selected = Some(todos::MAX_DEPTH as u64);
    assert!(state.todo_key(key('A')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.todos.note.as_ref().unwrap().hint.contains("5 levels"));
    state.todos.selected = Some(todos::MAX_DEPTH as u64 - 1);
    state.todo_key(key('A'));
    assert!(matches!(state.todos.mode, Mode::Input { .. }));
}

/// A draft kept before the list reached a limit comes back when its key is pressed, with
/// the limit as the box's refusal: text only the box can show is never out of reach.
#[test]
fn a_kept_draft_comes_back_at_the_limits() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "kept");
    state.todo_key(bare(BareKey::Esc));
    state.todos.list.items = (1..=todos::MAX_OPEN as u64)
        .map(|id| item(id, "open", false, vec![]))
        .collect();
    assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
    assert!(state.todo_key(key('a')));
    assert_eq!(draft(&state), Some("kept"));
    assert!(
        refusal(&state).unwrap().contains("200 open todos"),
        "{:?}",
        refusal(&state)
    );
    assert!(state.todo_key(bare(BareKey::Enter)));
    assert_eq!(draft(&state), Some("kept"));
    state.todo_key(bare(BareKey::Esc));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state
        .todos
        .drafts
        .iter()
        .any(|(_, input)| input.get_text() == "kept"));

    // A subtask whose parent has been pushed down to the depth limit meanwhile.
    let mut state = two_todos();
    let parent = todos::MAX_DEPTH as u64 - 1;
    let mut chain = item(parent, "level", false, vec![]);
    for id in (1..parent).rev() {
        chain = item(id, "level", false, vec![chain]);
    }
    state.todos.list.items = vec![chain.clone()];
    state.todos.selected = Some(parent);
    state.todo_key(key('A'));
    typed(&mut state, "child");
    state.todo_key(bare(BareKey::Esc));
    state.todos.list.items = vec![item(100, "above", false, vec![chain])];
    assert!(state.todo_key(key('A')));
    assert_eq!(draft(&state), Some("child"));
    assert!(
        refusal(&state).unwrap().contains("5 levels"),
        "{:?}",
        refusal(&state)
    );
}

/// Tab and Shift+Tab walk the ring from the box as from the list, and the box is still
/// there with its text when the keys come back.
#[test]
fn tab_in_the_box_walks_the_ring_and_keeps_the_draft() {
    let mut state = two_todos();
    state.todo_key(key('a'));
    typed(&mut state, "x");
    assert!(state.todo_key(bare(BareKey::Tab)));
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    assert_eq!(draft(&state), Some("x"));
    assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert_eq!(draft(&state), Some("x"));
}

/// Tab and Shift+Tab walk the ring from a move and from the project picker as from the
/// box, and the pick is still there when the keys come back; a question is dropped first.
#[test]
fn tab_walks_the_ring_from_a_move_and_a_project_pick_and_keeps_them() {
    let mut state = parent_and_child();
    state.todos.selected = Some(2);
    state.todo_key(key('m'));
    assert!(state.todo_key(bare(BareKey::Tab)));
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));
    assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));

    state.tabs_focused = false;
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('p'));
    assert!(matches!(state.todos.mode, Mode::Projects { .. }));
    assert!(state.todo_key(bare(BareKey::Tab)));
    assert_eq!(state.ring_yields, [wire::STOP_TABS, wire::STOP_TABS]);
    assert!(matches!(state.todos.mode, Mode::Projects { .. }));
    assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Projects { .. }));

    state.tabs_focused = false;
    state.todo_key(bare(BareKey::Esc));
    state.todos.selected = Some(1);
    state.todo_key(key('d'));
    assert!(matches!(state.todos.mode, Mode::Delete { .. }));
    assert!(state.todo_key(bare(BareKey::Tab)));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(state.ring_yields.len(), 3);
    assert_eq!(texts(&state), ["parent", "child"], "nothing deleted");
}

/// A move or a box about a todo that another writer deleted: the move goes, saying why,
/// and the box keeps the user's text as a new todo, saying so before Enter adds it.
#[test]
fn a_todo_deleted_elsewhere_ends_its_move_and_its_box_keeps_the_text() {
    let mut state = parent_and_child();
    state.todos.selected = Some(2);
    state.todo_key(key('m'));
    assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));
    state.todo_rows_changed(DataWatchId::new(1), watched(1));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);

    let mut state = parent_and_child();
    state.todos.selected = Some(2);
    state.todo_key(key('e'));
    typed(&mut state, " reworded");
    state.todo_rows_changed(DataWatchId::new(1), watched(1));
    assert!(matches!(
        state.todos.mode,
        Mode::Input {
            action: InputAction::Add { parent_id: None },
            ..
        }
    ));
    assert_eq!(draft(&state), Some("child reworded"));
    assert!(refusal(&state).unwrap().contains("Enter adds it as new"));
    assert_eq!(state.todo_cursor(), None);
}

/// The box marks what it is about - an edit's item, a subtask's parent - wherever the
/// selection goes meanwhile, and says which kind of entry it is.
#[test]
fn input_mode_marks_its_target() {
    let mut state = parent_and_child();
    state.todo_key(key('A'));
    assert_eq!(state.todo_cursor(), Some(1));
    state.todos.selected = Some(2);
    assert_eq!(state.todo_cursor(), Some(1));
    let (row, _) = state.input_divider(80).unwrap();
    assert!(row.line.contains("subtask of \"parent\": "), "{}", row.line);
    let (row, _) = state.input_divider(24).unwrap();
    assert!(row.line.contains("subtask: "), "{}", row.line);
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('a'));
    assert_eq!(state.todo_cursor(), None);
    assert!(state
        .input_divider(80)
        .unwrap()
        .0
        .line
        .contains("new top-level todo: "));
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('e'));
    assert_eq!(state.todo_cursor(), Some(2));
}

/// Success notes last until the next key - not until the next change to the list, which
/// for this pane's own write is the echo of it a moment later.
#[test]
fn a_success_note_outlives_the_watch_echo_and_goes_with_the_next_key() {
    let (backend, mut state) = stored(|backend| {
        put(backend, "one", None);
        put(backend, "two", None);
    });
    state.todo_key_with(&backend, key(' '));
    assert!(state.todos.note.is_some());
    let echo = backend
        .fetch(&FetchRequest::entity("todo").filter(Pred::eq("root", "/work")))
        .unwrap();
    assert!(state.todo_rows_changed(store::LOCAL_WATCH_ID, echo));
    assert!(state.todos.note.is_some());
    assert!(state.todo_key_with(&backend, key('j')));
    assert!(state.todos.note.is_none());
}

/// A note or a problem leads the hint line in its own colour and leaves the keys after
/// it, rather than taking the whole row.
#[test]
fn an_error_leaves_the_keys_on_show() {
    let mut state = two_todos();
    state.todos.note = Some(Note::error("boom"));
    let row = state.todo_divider(80, 0, 0);
    assert!(row.error);
    let bright = row.bright.clone().unwrap();
    assert_eq!(
        row.line
            .chars()
            .skip(bright.start)
            .take(bright.len())
            .collect::<String>(),
        "boom"
    );
    assert!(row.line.contains("boom · Space done"), "{}", row.line);
    state.todos.note = None;
    state.todos.status = "the store is down".into();
    let row = state.todo_divider(80, 0, 0);
    assert!(row.error);
    assert!(
        row.line
            .contains("the store is down · Ctrl-r retry · Space done"),
        "{}",
        row.line
    );
    state.todos.status.clear();
    let row = state.todo_divider(80, 0, 0);
    assert!(row.bright.is_none() && !row.error);
}

/// With no project the list says how to get one, the hint line offers nothing that needs
/// one, and `a` says why it cannot add rather than doing nothing.
#[test]
fn the_empty_list_says_what_to_do() {
    let mut state = State {
        ..Default::default()
    };
    let joined = |state: &State| {
        state
            .todo_empty_text()
            .into_iter()
            .map(|(line, _)| line)
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(joined(&state).contains("yinz action project-remember NAME --root DIR"));
    let hints = state.todo_divider(80, 0, 0).line;
    assert!(
        !hints.contains("a add") && hints.contains("Ctrl-r reload"),
        "{hints}"
    );
    assert!(state.todo_key(key('a')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state
        .todos
        .note
        .as_ref()
        .unwrap()
        .lead
        .contains("no projects"));

    state.todos.projects = projects::Registry::from([("api".into(), project("api", "/api"))]);
    assert!(joined(&state).contains("p to pick one"));
    // Not a folder the user cannot see: the one the panel happened to be opened in.
    assert!(joined(&state).contains("no project picked"));
    assert!(
        !joined(&state).contains("this folder"),
        "{}",
        joined(&state)
    );
    assert!(state
        .todo_divider(80, 0, 0)
        .line
        .contains("p pick a project"));

    state.todos.project_root = Some("/api".into());
    assert!(joined(&state).contains("a to add one"));
    state.todos.list.items = vec![item(1, "done", true, vec![item(2, "done", true, vec![])])];
    assert!(
        joined(&state).contains("2 done todos hidden"),
        "{}",
        joined(&state)
    );
    assert!(joined(&state).contains("v to show"));
}

/// The empty list's lines wrap to the pane rather than being cut at its edge, where the
/// command to run and the key to press are: every word is on screen, and none is split.
#[test]
fn the_empty_text_wraps_to_the_pane_rather_than_cutting() {
    let none = State {
        ..Default::default()
    };
    let mut unpicked = State {
        ..Default::default()
    };
    unpicked.todos.projects = projects::Registry::from([("api".into(), project("api", "/api"))]);
    let mut hidden = two_todos();
    hidden.todos.list.items = vec![item(1, "one", true, vec![]), item(2, "two", true, vec![])];
    let mut empty = two_todos();
    empty.todos.list.items.clear();
    empty.todos.selected = None;
    let words = |lines: &[(String, bool)]| -> Vec<String> {
        lines
            .iter()
            .flat_map(|(line, _)| line.split_whitespace().map(str::to_owned))
            .collect()
    };
    for mut state in [none, unpicked, hidden, empty] {
        let lines = state.todo_empty_text();
        for cols in [16, 22, 24, 34, 46] {
            let rows = empty_text_rows(&lines, cols);
            assert!(
                rows.iter().all(|(row, _)| row.width() <= cols),
                "{cols}: {rows:?}"
            );
            assert_eq!(words(&rows), words(&lines), "{cols}: {rows:?}");
        }
        state.render_todos(12, 22);
    }
    let none = State::default();
    let rows = empty_text_rows(&none.todo_empty_text(), 22);
    assert_eq!(
        rows,
        [
            ("no projects yet".to_owned(), false),
            ("start an agent in a".to_owned(), true),
            ("git checkout,".to_owned(), true),
            ("or: yinz action".to_owned(), true),
            ("project-remember NAME".to_owned(), true),
            ("--root DIR".to_owned(), true),
        ]
    );
    for cols in [16, 22, 24, 34, 46] {
        let joined = empty_text_rows(&none.todo_empty_text(), cols)
            .into_iter()
            .map(|(row, _)| row)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("yinz action project-remember NAME --root DIR"),
            "{cols}: {joined}"
        );
    }
}

/// A panel opened outside every project opens on none, rather than on whichever project's
/// name sorts first; the project shown is named above the list, where nothing replaces it.
#[test]
fn a_folder_outside_every_project_opens_on_none_and_a_project_is_named_above_its_list() {
    let mut state = State {
        ..Default::default()
    };
    state.todos.initial_cwd = "/elsewhere".into();
    state.take_todo_projects(
        &Host::shared(),
        projects::Registry::from([
            ("alpha".into(), project("alpha", "/alpha")),
            ("beta".into(), project("beta", "/beta")),
        ]),
    );
    assert_eq!(state.todos.project_root, None);
    assert!(state.todo_header(40).is_none());
    state.todo_key(key('a'));
    assert!(state
        .todos
        .note
        .as_ref()
        .unwrap()
        .hint
        .contains("p to pick"));

    state.todos.project_root = Some("/beta".into());
    state.todos.status = "boom".into();
    let header = state.todo_header(40).unwrap();
    assert_eq!(header.line, "Project: beta · p switch");
    assert_eq!(header.bright, Some(0..13));
    state.render_todos(12, 40);
    assert_eq!(state.todos.last_header_rows, 1);
    assert_eq!(state.todos.last_list_rows, 10);
    // Too short to spare the row: the list keeps it.
    state.render_todos(2, 40);
    assert_eq!(state.todos.last_header_rows, 0);

    // `p` switches only from the list: in a box it is a letter, in a move or a question a
    // key that does something else, and the header says so by not offering it.
    let plain = |state: &State| {
        let header = state.todo_header(40).unwrap();
        assert_eq!(header.line, "Project: beta");
        assert_eq!(header.bright, Some(0..13));
    };
    state.todos.status.clear();
    state.todo_key(key('a'));
    assert!(matches!(state.todos.mode, Mode::Input { .. }));
    plain(&state);
    state.todo_key(bare(BareKey::Esc));
    state.todos.mode = Mode::Move {
        id: 1,
        rev: DataRevision::new(1),
        target: None,
    };
    plain(&state);
    state.todos.mode = Mode::Delete {
        id: 1,
        plan: Vec::new(),
        descendants: 0,
    };
    plain(&state);
    state.todos.mode = Mode::Normal;
    assert_eq!(
        state.todo_header(40).unwrap().line,
        "Project: beta · p switch"
    );
}

/// Ctrl-r reads the list again in place: the cursor stays on its todo, or goes to the
/// nearest one left, rather than back to the top.
#[test]
fn ctrl_r_reloads_in_place_and_the_hint_line_names_it() {
    let (backend, mut state) = stored(|backend| {
        for at in 0..25 {
            put(backend, &format!("todo {at}"), None);
        }
    });
    let twentieth = state.todos.list.items[19].id;
    state.todos.selected = Some(twentieth);
    assert!(state.todo_divider(200, 0, 0).line.contains("Ctrl-r reload"));
    let reload = key('r').with_ctrl_modifier();
    assert!(state.todo_key_with(&backend, reload.clone()));
    assert_eq!(state.todos.selected, Some(twentieth));
    backend
        .apply(vec![Mutation::delete("todo", Pred::eq("id", twentieth))])
        .unwrap();
    state.todo_key_with(&backend, reload);
    assert_eq!(state.todos.selected, Some(state.todos.list.items[19].id));
    assert_eq!(state.todos.list.items.len(), 24);
}

#[test]
fn a_failed_reload_keeps_the_rows_on_screen() {
    let mut state = two_todos();
    state.todos.selected = Some(2);
    let before = state.todos.list.clone();
    assert!(state.todo_key(key('r').with_ctrl_modifier()));
    assert_eq!(state.todos.list, before);
    assert_eq!(state.todos.selected, Some(2));
    assert!(!state.todos.status.is_empty());
    assert!(state.todo_divider(80, 0, 0).line.contains("Ctrl-r retry"));
}

/// With no registry read - the store refused it as the panel opened - Ctrl-r tries the
/// registry again rather than reloading a list no project has, which would only clear the
/// error saying why; and the list says the projects could not be read, not that there are
/// none.
#[test]
fn ctrl_r_after_a_registry_error_retries_the_registry() {
    let mut state = State {
        ..Default::default()
    };
    let joined = |state: &State| {
        state
            .todo_empty_text()
            .into_iter()
            .map(|(line, _)| line)
            .collect::<Vec<_>>()
            .join(" ")
    };
    state.open_todos("/work".into());
    assert!(!state.todos.status.is_empty());
    assert!(state.todos.project_watch.is_none());
    assert_eq!(state.todos.project_root, None);
    assert!(
        !joined(&state).contains("no projects yet"),
        "{}",
        joined(&state)
    );
    state.todo_key(key('a'));
    assert_eq!(
        state.todos.note.as_ref().unwrap().lead,
        "projects not loaded"
    );
    let reload = key('r').with_ctrl_modifier();
    assert!(state.todo_key_with(&Host::shared(), reload.clone()));
    assert!(!state.todos.status.is_empty());
    assert!(state.todos.project_watch.is_none());

    let backend = store::Local::shared("hocus-agents");
    backend.define_all(&store::shared_defs()).unwrap();
    backend
        .apply(vec![projects::to_mutation(&project("work", "/work"))])
        .unwrap();
    assert!(state.todo_key_with(&backend, reload));
    assert!(state.todos.project_watch.is_some());
    assert_eq!(state.todos.projects.len(), 1);
    assert_eq!(state.todos.project_root.as_deref(), Some("/work"));
    assert!(state.todos.status.is_empty(), "{}", state.todos.status);
    assert!(
        !joined(&state).contains("no projects yet"),
        "{}",
        joined(&state)
    );
}

/// The todo being moved keeps a mark while the cursor is off choosing its parent; the hint
/// line names it by its text; a todo with nowhere to go does not open a move at all; and
/// a move to where it already is writes nothing.
#[test]
fn a_move_marks_its_source_and_a_lone_todo_has_nowhere_to_go() {
    let mut state = two_todos();
    state.todo_key(key('m'));
    assert!(matches!(
        state.todos.mode,
        Mode::Move {
            id: 1,
            target: Some(2),
            ..
        }
    ));
    let hints = state.todo_divider(80, 0, 0).line;
    assert!(
        hints.starts_with("─ Enter nest · t top · Esc cancel · moving \"one\""),
        "{hints}"
    );
    let lines = todo_lines(&todo_rows(&state.todos.list, false), 40);
    assert!(line_body(&lines[0], false, true)
        .text
        .starts_with("↕[ ] one"));
    assert!(line_body(&lines[1], true, false)
        .text
        .starts_with(">[ ] two"));
    state.todo_key(key('t'));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert_eq!(state.todos.note.as_ref().unwrap().lead, "already top level");

    state.todos.list.items.pop();
    assert!(state.todo_key(key('m')));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state
        .todos
        .note
        .as_ref()
        .unwrap()
        .lead
        .starts_with("nowhere to move it"));
}

/// A move's ways out fit the narrowest half, `t` - the only way to the top level - always
/// among them, and the scroll readout gives way to them rather than crowding them out.
#[test]
fn a_move_keeps_its_top_level_key_on_the_narrowest_half() {
    let mut state = two_todos();
    state.todo_key(key('m'));
    for cols in 22..=60 {
        for (above, below) in [(0, 0), (3, 0), (12, 40)] {
            let line = state.todo_divider(cols, above, below).line;
            assert_eq!(line.width(), cols, "{cols}: {line}");
            assert!(line.starts_with("─ Enter nest · t top"), "{cols}: {line}");
            if cols >= 35 {
                assert!(line.contains("t top · Esc cancel"), "{cols}: {line}");
            }
            if cols == 60 {
                assert!(
                    line.ends_with(&scroll_readout(above, below)),
                    "{cols}: {line}"
                );
            }
        }
    }
}

/// Every note the tab says fits the narrowest half whole: the item's excerpt goes before a
/// word is cut, then the count is shortened, and no row ends in a dangling `+`.
#[test]
fn every_note_fits_the_narrowest_half_whole() {
    let long = "Old idea about the resize loop";
    let mut notes = vec![
        Note::done("deleted", long, 2),
        Note::done(
            "moved",
            "a todo with a name much too long for any narrow pane",
            0,
        ),
        Note::error(GONE),
        Note::error("changed elsewhere").then("list reloaded, try again"),
    ];
    let state = two_todos();
    notes.extend([3, 1, 0].map(|count| state.completed_note(long, count)));
    // Each refusal, as the keys that meet it make it.
    let refused = |mut state: State, keys: &[KeyWithModifier]| -> Note {
        for key in keys {
            state.todo_key(key.clone());
        }
        state.todos.note.take().expect("a refusal")
    };
    let mut unpicked = State::default();
    unpicked.todos.projects = projects::Registry::from([("api".into(), project("api", "/api"))]);
    let mut unloaded = State::default();
    unloaded.todos.status = "the store is down".into();
    let mut full = two_todos();
    full.todos.list.items = (1..=todos::MAX_OPEN as u64)
        .map(|id| item(id, "open", false, vec![]))
        .collect();
    let mut deep = two_todos();
    let mut chain = item(todos::MAX_DEPTH as u64, "deepest", false, vec![]);
    for id in (1..todos::MAX_DEPTH as u64).rev() {
        chain = item(id, "level", false, vec![chain]);
    }
    deep.todos.list.items = vec![chain];
    deep.todos.selected = Some(todos::MAX_DEPTH as u64);
    let mut lone = two_todos();
    lone.todos.list.items.pop();
    let mut child = parent_and_child();
    child.todos.selected = Some(2);
    notes.extend([
        refused(State::default(), &[key('a')]),
        refused(unpicked, &[key('a')]),
        refused(unloaded, &[key('a')]),
        refused(full, &[key('a')]),
        refused(deep, &[key('A')]),
        refused(lone, &[key('m')]),
        refused(two_todos(), &[key('m'), key('t')]),
        refused(child, &[key('m'), bare(BareKey::Enter)]),
    ]);
    let mut state = two_todos();
    for note in notes {
        state.todos.note = Some(note);
        for cols in 24..=34 {
            let text = state.todos.note.as_ref().unwrap().text(cols - 4);
            assert!(text.width() <= cols - 4, "{cols}: {text}");
            assert!(!text.ends_with('+'), "{cols}: {text}");
            let line = state.todo_divider(cols, 3, 9).line;
            assert!(line.contains(&text), "{cols}: {text} in {line}");
        }
    }
}

#[test]
fn picker_navigation_restores_cursor_scrolling_even_at_boundaries() {
    for projects in [true, false] {
        for (key, start, expected) in [
            (BareKey::Up, 5, 4),
            (BareKey::Char('k'), 0, 0),
            (BareKey::Down, 5, 6),
            (BareKey::Char('j'), 9, 9),
            (BareKey::Up, 0, 0),
            (BareKey::Char('k'), 5, 4),
            (BareKey::Down, 9, 9),
            (BareKey::Char('j'), 5, 6),
        ] {
            let mut state = State::default();
            state.todos.list.items = (0..=10).map(|id| item(id, "todo", false, vec![])).collect();
            state.todos.projects = (0..10)
                .map(|id| {
                    let name = format!("project {id}");
                    (name.clone(), project(&name, &format!("/work/{id}")))
                })
                .collect();
            state.todos.mode = if projects {
                Mode::Projects { selected: start }
            } else {
                Mode::Move {
                    id: 10,
                    rev: DataRevision::new(10),
                    target: Some(start as u64),
                }
            };
            state.render_todos(4, 80);
            state.todo_mouse(if start == 0 {
                Mouse::ScrollDown(20)
            } else {
                Mouse::ScrollUp(20)
            });
            state.render_todos(4, 80);
            assert!(state.todos.list_scroll.is_some());
            assert!(state.todo_key(KeyWithModifier::new(key)));
            assert_eq!(state.todos.list_scroll, None);
            state.render_todos(4, 80);
            if projects {
                assert!(
                    matches!(state.todos.mode, Mode::Projects { selected } if selected == expected)
                );
                assert!(state
                    .todos
                    .rendered_projects
                    .contains(&format!("/work/{expected}")));
            } else {
                assert!(
                    matches!(state.todos.mode, Mode::Move { target: Some(id), .. } if id == expected as u64)
                );
                assert!(state.todos.rendered_ids.contains(&(expected as u64)));
            }
        }
    }
}

/// The wheel stops at the end of the list, so turning it back moves the list at once, and
/// the readout says how much is below as well as above.
#[test]
fn a_wheel_past_the_end_banks_no_travel_and_the_divider_counts_both_ends() {
    let mut state = State::default();
    state.todos.list.items = (1..=30).map(|id| item(id, "todo", false, vec![])).collect();
    state.todos.selected = Some(1);
    state.render_todos(10, 40);
    assert_eq!(state.todos.last_list_rows, 9);
    assert!(state.todo_divider(40, 0, 21).line.ends_with(" ↓21 "));
    state.todo_mouse(Mouse::ScrollDown(50));
    state.render_todos(10, 40);
    assert_eq!(state.todos.last_window, 21);
    assert_eq!(state.todos.list_scroll, Some(21));
    state.todo_mouse(Mouse::ScrollUp(1));
    state.render_todos(10, 40);
    assert_eq!(state.todos.last_window, 20);

    state.todos.mode = Mode::Projects { selected: 0 };
    state.todos.projects = (0..30)
        .map(|id| {
            let name = format!("project {id:02}");
            (name.clone(), project(&name, &format!("/work/{id}")))
        })
        .collect();
    state.todos.list_scroll = None;
    state.todo_mouse(Mouse::ScrollDown(99));
    state.render_todos(10, 40);
    assert_eq!(state.todos.list_scroll, Some(30 - 8));
}

/// The wheel's place in the picker is not a place in the list: leaving the picker without
/// a pick, whichever way, puts the list back around its cursor.
#[test]
fn leaving_the_picker_drops_its_wheel_scroll() {
    for exit in ["Esc", "Enter", "hide"] {
        let mut state = State {
            ..Default::default()
        };
        state.todos.project_root = Some("/work".into());
        state.todos.list.items = (1..=100)
            .map(|id| item(id, "todo", false, vec![]))
            .collect();
        state.todos.selected = Some(80);
        // Enter leaves without a pick only where there is nothing to pick.
        if exit != "Enter" {
            state.todos.projects = (0..30)
                .map(|id| {
                    let name = format!("project {id:02}");
                    (name.clone(), project(&name, &format!("/work/{id}")))
                })
                .collect();
        }
        state.render_todos(10, 40);
        state.todo_key(key('p'));
        state.todo_mouse(Mouse::ScrollDown(15));
        state.render_todos(10, 40);
        assert!(state.todos.list_scroll.is_some(), "{exit}");
        match exit {
            "Esc" => assert!(state.todo_key(bare(BareKey::Esc))),
            "Enter" => assert!(state.todo_key(bare(BareKey::Enter))),
            _ => state.hide_the_panel(),
        }
        assert!(matches!(state.todos.mode, Mode::Normal), "{exit}");
        assert_eq!(state.todos.list_scroll, None, "{exit}");
        state.render_todos(10, 40);
        assert!(state.todos.rendered_ids.contains(&80), "{exit}");
    }
}

/// Every key a mode answers is on its sheet.
#[test]
fn the_key_sheet_names_every_key_each_mode_binds() {
    let names = |state: &State| -> Vec<String> {
        state
            .todo_key_sheet()
            .into_iter()
            .flat_map(|(keys, _)| keys.split(" / ").map(str::to_owned).collect::<Vec<_>>())
            .collect()
    };
    let mut state = parent_and_child();
    let expect = |state: &State, keys: &[&str]| {
        let names = names(state);
        for key in keys {
            assert!(names.iter().any(|name| name == key), "{key}: {names:?}");
        }
    };
    expect(
        &state,
        &[
            "Space",
            "Enter",
            "e",
            "a",
            "A",
            "d",
            "n",
            "m",
            "p",
            "v",
            "j",
            "k",
            "↑",
            "↓",
            "Ctrl-r",
            "Tab",
            "Shift+Tab",
            "Esc",
            // The box's, which has no sheet of its own: `?` is typed there.
            "Alt+Enter",
            "Shift+Enter",
            "Home",
            "End",
            "Backspace",
            "Ctrl-w",
            "Ctrl-u",
            "Ctrl-z",
        ],
    );
    state.todo_key(key('d'));
    expect(&state, &["y", "any other key"]);
    state.todo_key(key('n'));
    state.todo_key(key(' '));
    expect(&state, &["y", "any other key"]);
    state.todo_key(key('n'));
    state.todos.selected = Some(2);
    state.todo_key(key('m'));
    expect(&state, &["Enter", "t", "Esc", "j", "k", "Tab", "Shift+Tab"]);
    state.todo_key(bare(BareKey::Esc));
    state.todo_key(key('p'));
    expect(&state, &["Enter", "Esc", "j", "k", "Tab", "Shift+Tab"]);
}

/// `?` types into the box and opens the sheet on the list. The bar consumes it.
#[test]
fn a_question_mark_is_typed_into_the_box_and_opens_the_sheet_outside_it() {
    let mut state = two_todos();
    state.render(12, 40);
    assert!(state.on_key(key('?')));
    assert_eq!(state.key_sheet_scroll, Some(0));
    assert!(state
        .key_sheet()
        .iter()
        .any(|(keys, what)| keys == "Space" && what.contains("complete")));
    assert!(state.on_key(bare(BareKey::Esc)));
    assert_eq!(state.key_sheet_scroll, None);

    state.on_key(key('a'));
    state.render(12, 3);
    assert!(state.on_key(key('?')));
    state.on_key(bare(BareKey::Tab).with_shift_modifier());
    assert!(state.tabs_focused);
    assert!(state.on_key(key('?')));
    assert_eq!(draft(&state), Some("?"));
    assert!(state.tabs_focused);
    assert_eq!(state.key_sheet_scroll, None);
}

/// The bar consumes paste even when an input box remains open underneath.
#[test]
fn a_paste_on_the_bar_leaves_the_box_unchanged() {
    let mut state = two_todos();
    state.render(12, 40);
    assert!(state.on_key(key('a')));
    for ch in "Fix ".chars() {
        state.on_key(key(ch));
    }
    assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(!state.on_paste("flaky test"));
    assert_eq!(draft(&state), Some("Fix "));
    assert!(state.tabs_focused);
    assert!(state.on_key(bare(BareKey::Esc)));
    assert_eq!(draft(&state), Some("Fix "));
    assert!(state.on_key(bare(BareKey::Esc)));
    assert!(matches!(state.todos.mode, Mode::Normal));
    assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(!state.on_paste("ajk"));
    assert!(state.tabs_focused);
    assert!(matches!(state.todos.mode, Mode::Normal));
}

#[test]
fn a_vanished_selection_moves_to_its_nearest_surviving_neighbour() {
    let list = todos::List {
        items: vec![
            item(1, "first", false, vec![]),
            item(2, "parent", false, vec![item(3, "child", false, vec![])]),
            item(4, "last", false, vec![]),
        ],
        ..Default::default()
    };
    let before = todo_rows(&list, false);
    let without = |hidden: &[u64]| -> Vec<Row> {
        before
            .iter()
            .filter(|row| !hidden.contains(&row.id))
            .cloned()
            .collect()
    };
    assert_eq!(nearest_surviving(&before, &before, Some(2)), Some(2));
    assert_eq!(nearest_surviving(&before, &without(&[3]), Some(3)), Some(4));
    assert_eq!(
        nearest_surviving(&before, &without(&[2, 3]), Some(2)),
        Some(4)
    );
    assert_eq!(nearest_surviving(&before, &without(&[4]), Some(4)), Some(3));
    assert_eq!(nearest_surviving(&before, &before, Some(9)), Some(1));
    assert_eq!(nearest_surviving(&before, &[], Some(2)), None);
}

#[test]
fn completing_the_selected_todo_keeps_the_cursor_in_place() {
    let rows = |done: u64| DataRows {
        columns: ["id", "root", "text", "parent_id", "rev", "done"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        rows: (1..=3)
            .map(|id| {
                vec![
                    DataValue::Int(id),
                    DataValue::Text("/work".into()),
                    DataValue::Text(format!("todo {id}")),
                    DataValue::Null,
                    DataValue::Int(id),
                    DataValue::Bool(id as u64 == done),
                ]
            })
            .collect(),
        truncated: false,
        next_cursor: None,
    };
    for (selected, expected) in [(2, 3), (3, 2)] {
        let mut state = State {
            ..Default::default()
        };
        state.todos.project_root = Some("/work".into());
        state.todos.todo_watch = Some(DataWatchId::new(1));
        state.todo_rows_changed(DataWatchId::new(1), rows(0));
        state.todos.selected = Some(selected);
        state.todo_rows_changed(DataWatchId::new(1), rows(selected));
        assert_eq!(state.todos.selected, Some(expected));
    }
}

#[test]
fn items_render_as_markdown_and_wrap_under_their_text() {
    let list = todos::List {
        items: vec![item(
            1,
            "**bold** and `code`",
            false,
            vec![item(2, "one two three four five six", true, vec![])],
        )],
        ..Default::default()
    };
    let lines = todo_lines(&todo_rows(&list, true), 16);
    let texts: Vec<_> = lines.iter().map(|line| line.line.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "[ ] bold and",
            "    code",
            "  [x] one two",
            "      three",
            "      four five",
            "      six"
        ]
    );
    assert!(lines[0].line.styles[4].bold);
    assert!(!lines[0].line.styles[9].bold);
    assert_eq!(
        lines
            .iter()
            .map(|line| (line.id, line.first))
            .collect::<Vec<_>>(),
        [
            (1, true),
            (1, false),
            (2, true),
            (2, false),
            (2, false),
            (2, false)
        ]
    );
    assert!(lines
        .iter()
        .all(|line| line.line.text.width() < 16 && line.done == (line.id == 2)));
}

#[test]
fn wrapped_items_scroll_by_row_and_click_through_continuations() {
    let mut state = State::default();
    state.todos.list.items = vec![
        item(1, "short", false, vec![]),
        item(2, "one two three four five six seven eight", false, vec![]),
        item(3, "last", false, vec![]),
    ];
    state.todos.selected = Some(1);
    state.render_todos(20, 16);
    assert_eq!(state.todos.rendered_ids, [1, 2, 2, 2, 2, 3]);

    state.todo_mouse(Mouse::LeftClick(3, 0));
    assert_eq!(state.todos.selected, Some(2));

    // Three list rows: the tall item shows its head rather than being centred past it.
    state.render_todos(4, 16);
    assert_eq!(state.todos.last_window, 1);
    assert_eq!(state.todos.rendered_ids, [2, 2, 2]);

    state.todos.selected = Some(3);
    state.render_todos(4, 16);
    assert_eq!(state.todos.rendered_ids, [2, 2, 3]);

    // With the project's row above the list, a click is a row lower, and one on the
    // project's row is nobody's.
    state.todos.project_root = Some("/work".into());
    state.render_todos(20, 16);
    assert_eq!(state.todos.last_header_rows, 1);
    assert!(!state.todo_mouse(Mouse::LeftClick(0, 0)));
    state.todo_mouse(Mouse::LeftClick(1, 0));
    assert_eq!(state.todos.selected, Some(1));
}

#[test]
fn project_picker_preserves_a_root_across_rename() {
    let mut state = State::default();
    state.todos.initial_cwd = "/work/nested/src".into();
    state.todos.projects = projects::Registry::from([
        ("outer".into(), project("outer", "/work")),
        ("nested".into(), project("nested", "/work/nested")),
    ]);
    let selected = projects::for_cwd(
        &state.todos.projects,
        &Location::local(&state.todos.initial_cwd),
    )
    .unwrap();
    assert_eq!(selected.name, "nested");
    state.todos.project_root = Some(selected.root.clone());
    state.todos.projects =
        projects::Registry::from([("renamed".into(), project("renamed", "/work/nested"))]);
    assert_eq!(state.todo_project_name(), "renamed");
}
