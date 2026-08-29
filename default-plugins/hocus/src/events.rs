use crate::message::{Ask, Message, Status, StreamState};
use crate::*;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

impl State {
    pub(crate) fn is_toggle_key(key: &KeyWithModifier) -> bool {
        key.bare_key == BareKey::Char('`') && key.has_modifiers(&[KeyModifier::Ctrl])
    }

    /// Ctrl-o: move the conversation clear of a tiled pane - see `State::reposition`.
    pub(crate) fn is_reposition_key(key: &KeyWithModifier) -> bool {
        key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl])
    }

    /// The interrupt. Split out from Esc, which now only ever hides.
    pub(crate) fn is_interrupt_key(key: &KeyWithModifier) -> bool {
        key.bare_key == BareKey::Char('c') && key.has_modifiers(&[KeyModifier::Ctrl])
    }

    /// Re-resolve the config after Zellij reports the KDL block changed.
    ///
    /// Gated on `self.granted` — before the grant `self.env` is empty, and
    /// resolving against it would replace a "waiting for permission" state with
    /// a spurious "no API key" banner.
    pub(crate) fn reconfigure(&mut self) {
        if !self.granted {
            return;
        }
        self.cfg = Config::resolve(&self.kdl, &self.env);
    }

    /// Hocus is written for a *shared* plugin: one instance serving every
    /// attached client. Zellij only does that when the KDL block asks for it,
    /// and silently gives each client its own instance otherwise — which looks
    /// fine alone and splits into unrelated conversations the moment someone
    /// else attaches. Say so rather than let that be discovered.
    ///
    /// `Config::resolve` ignores the key, so this is the only thing reading it.
    pub(crate) fn warn_if_not_shared(&mut self) {
        if self.kdl.get("shared").map(|v| v.trim()) == Some("true") {
            return;
        }
        self.messages.push(Message::notice(
            "hocus is missing `shared true` in its plugin configuration. Without it Zellij \
             gives every attached client its own copy of the plugin, and they will not share \
             a conversation."
                .to_owned(),
        ));
    }

    /// This client's scroll position: wrapped transcript lines hidden below the
    /// viewport, zero being pinned to the bottom.
    pub fn scroll_offset(&self) -> usize {
        self.view.scroll.get(&self.client).copied().unwrap_or(0)
    }

    /// Record where this client's viewport actually ended up, which the render
    /// is the only thing that knows: it is the one place the anchor and the
    /// clamp are applied. Without it a client held to the anchor still reads as
    /// pinned to the bottom, and the first PageDown would fall off it.
    pub(crate) fn set_scroll_offset(&mut self, lines: usize) {
        self.view.scroll.insert(self.client, lines);
    }

    /// The message whose first line this client's viewport is being held to, if
    /// any. `None` once they have scrolled for themselves — the anchor exists to
    /// save them a trip back up, not to take the scrollbar off them.
    pub(crate) fn anchor(&self) -> Option<usize> {
        (!self.view.released.contains(&self.client))
            .then_some(self.view.anchor)
            .flatten()
    }

    /// Pin *every* client back to the tail. The conversation changing is not
    /// one client's business, so nobody is left reading a stale middle of it,
    /// held to a reply that is no longer what is arriving.
    pub(crate) fn pin_to_bottom(&mut self) {
        self.view.pin();
    }

    pub(crate) fn on_permissions(&mut self, status: PermissionStatus) -> bool {
        // Either way we stop saying "waiting": this is the only answer we get.
        self.perms = true;
        self.cfg = match status {
            // Safe now — gated calls will actually be answered.
            PermissionStatus::Granted => {
                self.granted = true;
                self.warn_if_not_shared();
                // The agent daemon needs this grant only for the initial snapshot;
                // afterwards it follows Zellij's pane events.
                self.subscribe_to_agentd();
                self.env = get_session_environment_variables();
                // Here rather than in `load` for the same reason as the environment: a
                // value-returning gated call before the grant lands waits on a reply the host
                // never sends. Like the environment, it is fixed for the session.
                self.user_plugin_dir = get_user_plugin_dir();
                self.start_extension_scan();
                let cfg = Config::resolve(&self.kdl, &self.env);
                // After the environment is in, which `skills_dir` may expand
                // `~/` against; not gated on the config resolving, because a
                // missing API key is no reason to hide what `/skills` would
                // list.
                self.start_user_skill_scan();
                self.sync_project_skills();
                cfg
            },
            PermissionStatus::Denied => Err(ConfigError {
                message: "Permission denied.\n\nhocus needs web access (to reach the API) and \
                          read access to session environment variables (to find your API key). \
                          Grants are cached by the plugin's absolute .wasm path — delete this \
                          plugin's entry from permissions.kdl in Zellij's cache directory and \
                          reload to be asked again."
                    .to_owned(),
                base_url: api::DEFAULT_BASE_URL.to_owned(),
                model: api::DEFAULT_MODEL.to_owned(),
                api_key_env: api::DEFAULT_API_KEY_ENV.to_owned(),
            }),
        };
        true
    }

    pub(crate) fn on_tabs(&mut self, tabs: Vec<TabInfo>) -> bool {
        self.tab_projects
            .retain(|id, _| tabs.iter().any(|tab| tab.tab_id == *id));
        self.auto_tab_projects
            .retain(|id, _| tabs.iter().any(|tab| tab.tab_id == *id));
        self.tabs = tabs;
        // The column is placed against the screen, and this is the only event that reports its
        // size - there is no resize event for a plugin, and `render` is handed the pane's size
        // rather than the screen's.
        if let Some(active) = self.tabs.iter().find(|tab| tab.active) {
            let (rows, cols) = overlay::surface_of(active);
            let position = active.position;
            let resized = (rows, cols) != (self.surface_rows, self.surface_cols);
            if resized {
                self.surface_rows = rows;
                self.surface_cols = cols;
            }
            // Where this client is now, against where they were. The rule that reads it is in
            // `overlay::tab_to_follow`: only the client the panes are sitting with may move them,
            // because every client gets its own `TabUpdate` and obeying all of them puts two
            // clients on different tabs into a move loop with each other.
            let was_on = self.client_tabs.insert(self.client, position);
            // Not while *both* panes are set aside: a panel the user has put away does not chase
            // their tab switches, and comes back where it is asked for - see `on_panes`. A hidden
            // conversation beside a visible list still follows, or the tab switch strands the list
            // on the tab it was opened from.
            if !self.hidden || self.agents_visible {
                if let Some(target) = overlay::tab_to_follow(self.own_tab_index, was_on, position) {
                    self.follow_tab(target);
                }
            }
            // Outside the visibility gate, deliberately, where the tab-following above is not. The
            // surface is cached a few lines up whatever is on screen, and this event is the only
            // thing that ever reports it - so a resize seen while both panes were set aside is the
            // last word on the surface and would never be re-reported. Skipping the placement for
            // it would leave a pane revealed afterwards at the old size, in the old corner.
            //
            // Placing a hidden pane is not painting one: the server re-resolves a hidden overlay
            // like any other (`OverlayPanes::reapply_coordinates`) and gates the paint separately
            // (`stack_layer_ids`), and `hide_self_only` already places from behind the hide. It has
            // to be hocus that restates it - these coordinates are `Fixed`, because the panes are
            // anchored to the *bottom*, so nothing on the server can recompute them.
            //
            // `!placed` as well as `resized`: the first `TabUpdate` is the only one that ever
            // reports a *change* of display area, so a placement it could not land - our pane
            // not being on a tab yet - would otherwise never be retried. See `State::placed`.
            // `!agents_placed` for the same reason, about the other pane - see
            // `State::agents_placed`.
            if resized || !self.placed || !self.agents_placed {
                self.apply_overlay_geometry();
            }
        }
        // Rather than deriving the active project a second way here. Switching tabs
        // does not change which agent was focused last, but a tab that went away
        // can still change the answer.
        self.refresh_tab_projects();
        false
    }

    /// Look for extensions in the plugin folder, through the host's `/plugins` root. Answered
    /// at once; `on_extension_scan_result` keeps the shape the `find` this used to run had, one
    /// real path per line, because that is what `extensions::extension_files` reads.
    fn start_extension_scan(&mut self) {
        let dir = self.user_plugin_dir.trim().to_owned();
        if dir.is_empty() {
            return;
        }
        let Ok(entries) = tools::plugin_fs_list(tools::PLUGINS_ROOT) else {
            return;
        };
        let listing = tools::listing_names(tools::PLUGINS_ROOT, &entries)
            .iter()
            .map(|name| format!("{dir}/{name}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.on_extension_scan_result(Some(0), listing.as_bytes());
    }

    pub(crate) fn on_extension_scan_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: &[u8],
    ) -> bool {
        self.extension_files = if exit_code == Some(0) {
            extensions::extension_files(&self.user_plugin_dir, &String::from_utf8_lossy(stdout))
        } else {
            BTreeMap::new()
        };
        if self.extension_files.is_empty() {
            return false;
        }
        extensions::discover();
        self.extension_scan_due_ms = Some(self.mono_ms.saturating_add(PLUGIN_LOAD_MS));
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The user's own skills folder: the KDL `skills_dir`, or `hocus/skills`
    /// under the plugin folder the host reported. Empty when there is neither.
    ///
    /// Read straight from the block rather than through `Config`: the config
    /// is an `Err` without an API key, and the skills are worth listing
    /// regardless.
    pub(crate) fn skills_dir(&self) -> String {
        match self.kdl.get("skills_dir").map(|dir| dir.trim()) {
            Some(dir) if !dir.is_empty() => api::expand_home(dir, &self.env),
            _ => {
                let dir = self.user_plugin_dir.trim().trim_end_matches('/');
                if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/hocus/skills")
                }
            },
        }
    }

    /// Every name a skill may not take: a skill that shadowed a tool or one
    /// of the plugin's own commands would turn a typed command into a prompt.
    pub(crate) fn reserved_command_names(&self) -> BTreeSet<String> {
        tools::names()
            .into_iter()
            .chain(LOCAL_COMMANDS.iter().map(|name| (*name).to_owned()))
            .chain(self.extensions.names())
            .collect()
    }

    /// Read the user tier again. Nothing is cleared first: a scan that fails
    /// leaves what was loaded, and says so in `/skills`.
    pub(crate) fn start_user_skill_scan(&mut self) {
        let dir = self.skills_dir();
        if dir.is_empty() {
            self.skills.clear(skills::Source::User);
            return;
        }
        self.start_skill_scan(skills::Source::User, vec![dir]);
    }

    /// Read the project tier again if the active project's root changed since
    /// it was last read. The old project's skills are dropped at once rather
    /// than waiting for the scan: until it answers, a command from the project
    /// just left must not still work.
    ///
    /// Called wherever `active_project` is written. Not from the timer — an
    /// idle hocus has none running. Nothing before the grant, and in
    /// particular no root recorded: a tab update can land first, and a root
    /// remembered then would look already read to the sync the grant runs.
    pub(crate) fn sync_project_skills(&mut self) {
        if !self.granted {
            return;
        }
        let root = self
            .active_project
            .as_deref()
            .and_then(|name| self.projects.get(name))
            .map(|project| project.root.trim_end_matches('/').to_owned());
        if root == self.skills_project_root {
            return;
        }
        self.skills_project_root = root.clone();
        self.skills.clear(skills::Source::Project);
        if let Some(root) = root {
            self.start_skill_scan(
                skills::Source::Project,
                vec![format!("{root}/.hocus/skills")],
            );
        }
    }

    /// Ask the host to read `roots`. Answered in `on_skill_scan_result`; off
    /// wasm the call is a no-op and the argv is recorded for the tests.
    fn start_skill_scan(&mut self, source: skills::Source, roots: Vec<String>) {
        if !self.granted {
            return;
        }
        let argv = skills::scan_argv(&roots);
        #[cfg(test)]
        {
            self.last_skill_scan = Some(argv.clone());
        }
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        self.skills_scans_pending += 1;
        run_command_with_output_limit(
            &borrowed,
            BTreeMap::new(),
            std::path::PathBuf::from("."),
            BTreeMap::from([
                (skills::SCAN_CONTEXT.to_owned(), "true".to_owned()),
                (skills::SOURCE_CONTEXT.to_owned(), source.label().to_owned()),
            ]),
            skills::MAX_SCAN_OUTPUT,
        );
    }

    pub(crate) fn on_skill_scan_result(
        &mut self,
        source: skills::Source,
        exit_code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
    ) -> bool {
        self.skills_scans_pending = self.skills_scans_pending.saturating_sub(1);
        if exit_code == Some(0) {
            let listing = skills::parse_listing(source, &String::from_utf8_lossy(stdout));
            let reserved = self.reserved_command_names();
            self.skills.replace(source, listing, &reserved);
        } else {
            let stderr = String::from_utf8_lossy(stderr);
            let stderr = stderr.trim();
            self.skills.set_error(
                source,
                format!(
                    "the {} skills folder could not be read (exit code {}){}{stderr}",
                    source.label(),
                    exit_code.unwrap_or(-1),
                    if stderr.is_empty() { "" } else { ": " }
                ),
            );
        }
        self.log(
            "skills_scan",
            serde_json::json!({
                "source": source.label(),
                "exit_code": exit_code,
                "loaded": self.skills.names(),
            }),
        );
        // `/skills reload` is answered once, when the last scan it started
        // has landed.
        if self.skills_announce && self.skills_scans_pending == 0 {
            self.skills_announce = false;
            self.show_skills();
            return true;
        }
        false
    }

    pub(crate) fn on_sessions(&mut self, sessions: Vec<SessionInfo>) -> bool {
        let instances = sessions
            .into_iter()
            .find(|session| session.is_current_session)
            .into_iter()
            .flat_map(|session| session.plugins)
            .filter_map(|(id, plugin)| {
                let namespace = plugin.configuration.get("hocus_extension_name")?;
                let path = plugin.configuration.get("hocus_extension_path")?;
                (plugin.location == format!("file:{path}"))
                    .then(|| (id, (namespace.clone(), path.clone())))
            })
            .collect();
        if self.extension_instances == instances {
            return false;
        }
        for owner in self
            .extension_instances
            .iter()
            .filter(|(owner, identity)| instances.get(owner) != Some(identity))
            .map(|(owner, _)| *owner)
            .collect::<Vec<_>>()
        {
            self.extensions.remove_owner(owner);
        }
        self.extension_instances = instances;
        if !self.extension_files.is_empty() {
            extensions::discover();
        }
        false
    }

    pub(crate) fn on_extension_registration(&mut self, owner: u32, payload: Option<&str>) -> bool {
        let Some(payload) = payload else {
            return false;
        };
        // The skills too, so that the two cannot both claim a name: whichever
        // registered first keeps it, and the other is told.
        let reserved: BTreeSet<String> = tools::names()
            .into_iter()
            .chain(LOCAL_COMMANDS.iter().map(|name| (*name).to_owned()))
            .chain(self.skills.names())
            .collect();
        let names = match self.extensions.register(
            owner,
            payload,
            &self.extension_files,
            &self.extension_instances,
            &reserved,
        ) {
            Ok(names) => names,
            Err(error) => {
                self.log(
                    "extension_error",
                    serde_json::json!({"plugin_id": owner, "error": error}),
                );
                return false;
            },
        };
        let Some(path) = self
            .extension_files
            .values()
            .find(|path| self.extensions.owner_for_path(path) == Some(owner))
            .cloned()
        else {
            return false;
        };
        if self
            .plugin_load
            .as_ref()
            .is_some_and(|watch| watch.extension && watch.target == path)
        {
            return self.finish_extension_load(&path, &names);
        }
        let parked = self.parked.iter().position(|chat| {
            chat.plugin_load
                .as_ref()
                .is_some_and(|watch| watch.extension && watch.target == path)
        });
        parked.is_some_and(|index| {
            self.with_parked(index, |state| state.finish_extension_load(&path, &names))
        })
    }

    fn finish_extension_load(&mut self, path: &str, names: &[String]) -> bool {
        if !matches!(self.status, Status::AwaitingPluginLoad)
            || !self
                .plugin_load
                .as_ref()
                .is_some_and(|watch| watch.extension && watch.target == path)
        {
            return false;
        }
        self.plugin_load = None;
        self.status = Status::Idle;
        let result = if names.is_empty() {
            format!("loaded {path}; it registered no tools")
        } else {
            format!("loaded {path}; registered: {}", names.join(", "))
        };
        self.finish_head(result);
        self.pump_tools();
        true
    }

    pub(crate) fn on_extension_result(&mut self, owner: u32, payload: Option<&str>) -> bool {
        let Some(payload) = payload else {
            return false;
        };
        if payload.len() > extensions::MAX_RESULT_BYTES {
            return false;
        }
        let result = match serde_json::from_str::<extensions::ToolResult>(payload) {
            Ok(result) if result.version == extensions::VERSION => result,
            _ => return false,
        };
        match self.parked_with_call(&result.call_id) {
            Some(index) => {
                self.with_parked(index, |state| state.finish_extension_result(owner, result))
            },
            None => self.finish_extension_result(owner, result),
        }
    }

    fn finish_extension_result(&mut self, owner: u32, result: extensions::ToolResult) -> bool {
        if !matches!(self.status, Status::AwaitingExtension) {
            return false;
        }
        let Some(head) = self.pending.front() else {
            return false;
        };
        if head.id != result.call_id
            || self.extensions.tool(&head.name).map(|tool| tool.owner) != Some(owner)
        {
            return false;
        }
        self.status = Status::Idle;
        self.finish_head(log::truncate_str(&result.result, MAX_TOOL_OUTPUT));
        self.pump_tools();
        true
    }

    /// Watch our own pane for the two things only the server knows: which tab it is on, and
    /// whether it has been set aside.
    ///
    /// The reveal comes from *outside* - the `c` keybinding, or a `show_self` from a tool - and
    /// this is the only word hocus gets of it. `is_suppressed` is what an overlay reports while
    /// hidden, the same field a suppressed tab pane uses.
    ///
    /// The agent list is *not* discovered here. It announces itself with `AGENT_SELECTION` on
    /// load, which carries its plugin id and is what `agents_plugin` is already set from; a pane
    /// url cannot tell two instances apart, so there would be nothing gained by matching on one.
    pub(crate) fn on_panes(&mut self, manifest: PaneManifest) -> bool {
        let own_id = self.own_plugin_id;
        // An agent list the user closed has to be forgotten, or `open_agents_pane` keeps
        // revealing a pane that is not there and never opens a new one. Cleared from the whole
        // manifest rather than from our own tab: the two halves can be on different tabs for as
        // long as it takes a `follow_tab` to land.
        //
        // Whether it is *drawn* is read in the same pass, because the `a` keybinding can reveal
        // the list on its own while this half is still set aside - see below.
        let mut agents_visible = false;
        // Read in the same pass, and for the same reason: it is the only word this half gets about
        // what happened to the other one. `rejoin` reads it to tell a list the user summoned from
        // an apart list that was simply left behind - both bindings focus what they summon.
        let mut agents_focused = false;
        if let Some(agents_id) = self.agents_plugin {
            self.agents_tab_index = manifest.panes.iter().find_map(|(tab_index, panes)| {
                let agents = panes
                    .iter()
                    .find(|pane| pane.is_plugin && pane.id == agents_id)?;
                agents_visible = !agents.is_suppressed;
                agents_focused = agents.is_focused;
                Some(*tab_index)
            });
            if self.agents_tab_index.is_none() {
                self.agents_plugin = None;
                // A list that is gone is nothing to place. Said rather than left behind, because
                // the next one the user opens is a fresh pane at the centered default and the
                // retry that moves it out of the way reads this.
                self.agents_placed = true;
            }
        }
        // Held for `on_tabs`, which has no manifest of its own and has to know whether there is
        // anything on screen to carry to the tab the user switched to.
        self.agents_visible = agents_visible;
        for (tab_index, panes) in &manifest.panes {
            let Some(own) = panes
                .iter()
                .find(|pane| pane.is_plugin && pane.id == own_id)
            else {
                continue;
            };
            self.own_tab_index = Some(*tab_index);
            // Placement, or anything else outside hocus, may have renamed the
            // pane since our last render. Forgetting what we asked for is what
            // makes the next render assert the title again; see
            // `State::last_pane_title`.
            if self.last_pane_title.as_deref() != Some(own.title.as_str()) {
                self.last_pane_title = None;
            }
            // What Ctrl-o ranks, and what an anchored conversation follows. Read before the
            // placement retry below, so a manifest that both places and re-anchors does so once.
            self.tab_panes = panes.clone();
            self.track_anchor(panes);
            // The manifest is what makes a placement possible at all: it names the tab a pane is
            // on, which is the thing `change_overlay_coordinates` needs to find it. A pane still
            // sitting at the centered default the host gave it is placed here, whichever event
            // failed to place it first. Converges by construction - the call sets both flags.
            //
            // Either half, and the list's half is the one that had nowhere else to be caught:
            // `agents_tab_index` was read from this very manifest a few lines up, so this is the
            // first pass that can place a list which announced itself before it was on a tab.
            if !self.placed || !self.agents_placed {
                self.apply_overlay_geometry();
            }
            if own.is_suppressed {
                // the hide we asked for has landed. A suppressed conversation beside a visible
                // list is an ordinary state now, not half of anything: the two panes hide
                // separately, so there is nothing here to put back together.
                self.hide_pending = false;
                self.hidden = true;
            } else if self.open_hidden {
                // This instance was launched by the agent list's greeting, which asked for a list
                // and not for a conversation. Done here rather than where the greeting arrived,
                // because a `hide_self` issued before the pane is on a tab is dropped silently -
                // the manifest finding our pane is the first moment it can land.
                self.open_hidden = false;
                self.hide_self_only();
            } else if self.hide_pending {
                // A snapshot taken before our hide reached the server. Reading this as a reveal
                // would un-hide the pane the instant the user pressed Esc, so a hide we have
                // asked for but not yet seen suppresses exactly one thing: this inference.
            } else if self.hidden {
                self.reveal_self();
            }
            break;
        }
        self.rejoin_agents_pane(agents_focused);
        false
    }

    /// Put a parted pair back together, in whichever direction the user asked for.
    ///
    /// `follow_tab` moves the pair together and so cannot part them, but two paths outside hocus
    /// can. The `a` and `c` keybindings carry `move_to_focused_tab`, which moves only the pane the
    /// binding names - so a summon from another tab brings one half across and leaves the other
    /// where it was. `follow_tab` is no help afterwards: `own_tab_index` has already been set from
    /// this very manifest, so it reads as a pane that is where it belongs and returns.
    ///
    /// **Which half moved decides which way this goes**, and `overlay::rejoin` is where that is
    /// decided - see it for why the answer is "whichever half the server focused". Dragging
    /// unconditionally was wrong in one of the two cases: an unconditional drag took the list the
    /// user had just summoned with `a` straight off their tab again.
    ///
    /// Converges by construction either way: the move is what makes the next manifest report the
    /// two together, at which point this does nothing.
    fn rejoin_agents_pane(&mut self, agents_is_focused: bool) {
        let (Some(agents_id), Some(own_tab), Some(agents_tab)) = (
            self.agents_plugin,
            self.own_tab_index,
            self.agents_tab_index,
        ) else {
            return;
        };
        let agents_tab_is_someones = self.client_tabs.values().any(|tab| *tab == agents_tab);
        match overlay::rejoin(
            own_tab,
            agents_tab,
            agents_is_focused,
            agents_tab_is_someones,
        ) {
            // `follow_tab` carries both panes and restates both indices, so the list stays where
            // the user put it and the conversation arrives beside it.
            Some(overlay::Rejoin::Follow(tab_index)) => self.follow_tab(tab_index),
            Some(overlay::Rejoin::Drag(tab_index)) => {
                break_panes_to_tab_with_index(&[PaneId::Plugin(agents_id)], tab_index, false);
                self.agents_tab_index = Some(tab_index);
                // The destination resolves coordinates against its own surface, and the list
                // arrives with whatever the keybinding's launch gave it - the same restatement
                // `follow_tab` makes.
                self.apply_overlay_geometry();
            },
            None => {},
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if Self::is_toggle_key(&key) {
            self.hide_self_only();
            return false;
        }

        // This must be caught before TextInput sees it. Its bare-key arm
        // doesn't check for modifiers, so it would take Ctrl-l as a literal 'l'
        // and Alt-Enter as a submit.
        if key.bare_key == BareKey::Char('l') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            return self.new_chat();
        }
        // Taken ahead of the modal states below for the same reason Ctrl-l is: moving the pane
        // out of the way is wanted whatever is on screen in it, and the input box would swallow
        // the key. Nothing to redraw here - the move is the server's to paint.
        if Self::is_reposition_key(&key) {
            self.reposition();
            return false;
        }
        // The temporary lists are modal while open.
        if self.picker.is_some() {
            return self.picker_key(key);
        }
        // So is the tab bar, once Shift+Tab has put the highlight on it.
        if self.chat_tab_focus.is_some() {
            return self.chat_tab_key(key);
        }
        // Shift+Tab reaches us as Tab with the Shift modifier (CSI Z), and the
        // input box's Tab arm does not look at modifiers — so, like Ctrl-l
        // above, it has to be taken before the box would complete on it.
        if Self::is_bar_key(&key) {
            self.chat_tab_focus = Some(self.chat_tab);
            return true;
        }
        // Interrupt whatever is running. This used to be Esc's job, which meant a
        // streaming reply could not be put away without also being killed - the
        // state you are most likely to want out of the way was the one state where
        // hiding cost you the answer.
        if Self::is_interrupt_key(&key) {
            // A plan first: nothing is running while one waits, so it is the only
            // outstanding question, and `abort_tools` would push "tools aborted"
            // over an empty queue. Esc leaves the prompt standing, exactly as it
            // does for a tool waiting on y/n — hiding answers nothing.
            if self.awaiting_plan() {
                return self.decline_plan();
            }
            // Tools next, because they are the nearer thing: a stream and a tool
            // run can both be outstanding, and the tool is the one the prompt is
            // asking about.
            if self.tools_busy() {
                return self.abort_tools();
            }
            if self.is_streaming() {
                return self.cancel_stream();
            }
            // Nothing to interrupt, so it falls back to what Ctrl-c means in a
            // shell - and to what TextInput would have done with it anyway, before
            // this arm started taking the key first.
            self.input.clear();
            return true;
        }
        // The confirmation prompt owns y/n outright, for the same reason: they
        // would otherwise be typed into the input box.
        if (self.is_confirming() || self.awaiting_plan()) && key.has_no_modifiers() {
            let confirming = self.is_confirming();
            // The plan arm only fires on an empty box: with text already typed,
            // `n` is the start of a correction and must reach the editor instead
            // of being read as an answer to the approval prompt. `y` no longer
            // answers a plan at all — `/execute` does — so it is always typing.
            let input_is_empty = self.input.get_text().trim().is_empty();
            match key.bare_key {
                BareKey::Char('y') | BareKey::Char('Y') if confirming => {
                    return self.confirm_tool()
                },
                BareKey::Char('n') | BareKey::Char('N') if confirming => {
                    return self.decline_tool()
                },
                BareKey::Char('n') | BareKey::Char('N') if input_is_empty => {
                    return self.decline_plan()
                },
                _ => {},
            }
        }
        if key.bare_key == BareKey::Enter
            && (key.has_modifiers(&[KeyModifier::Alt]) || key.has_modifiers(&[KeyModifier::Shift]))
        {
            self.input.insert_char('\n');
            return true;
        }

        // History owns bare Up/Down. Page keys and the mouse still scroll.
        if key.has_no_modifiers() {
            let page = self.transcript_rows.saturating_sub(1).max(1);
            match key.bare_key {
                BareKey::PageUp => return self.scroll_up(page),
                BareKey::PageDown => return self.scroll_down(page),
                _ => {},
            }
        }

        match self.input.handle_key(key) {
            InputAction::Submit => self.submit(),
            InputAction::Cancel => {
                // Esc is unconditionally "put hocus away" now; Ctrl-c is the
                // interrupt. Nothing is torn down by hiding: a stream keeps
                // arriving into a suppressed pane and a tool call waiting on y/n
                // is still waiting when the pane comes back.
                //
                // Hidden, not unloaded; the agent list beside it, if there is one,
                // stays. The launch binding restores the same plugin from any tab.
                // Ctrl-l starts a new archived conversation.
                self.hide_self_only();
                false
            },
            InputAction::Continue => true,
            InputAction::Complete => self.complete_tool_name(),
            InputAction::NoAction => false,
        }
    }

    /// A bracketed paste, delivered whole rather than key by key. Newlines in
    /// it stay newlines, exactly as Alt-Enter produces them; a paste never
    /// submits, however it ends.
    pub(crate) fn on_paste(&mut self, text: String) -> bool {
        // The states that already take every key before the input box sees it
        // refuse a paste for the same reason.
        if self.picker.is_some()
            || self.chat_tab_focus.is_some()
            || self.is_confirming()
            || self.awaiting_plan()
        {
            return false;
        }
        self.input.insert_text(&text);
        true
    }

    /// Shift+Tab: the key that hands the keyboard to the tab bar.
    pub(crate) fn is_bar_key(key: &KeyWithModifier) -> bool {
        key.bare_key == BareKey::Tab && key.has_modifiers(&[KeyModifier::Shift])
    }

    /// Every key while the tab bar has the highlight. Unhandled keys are
    /// swallowed, as the picker's are: the input box is behind the bar, and a
    /// stray letter would be typed into it unseen.
    pub(crate) fn chat_tab_key(&mut self, key: KeyWithModifier) -> bool {
        let Some(selected) = self.chat_tab_focus else {
            return false;
        };
        // Ctrl-l is taken ahead of this, in `on_key`, and opens a new tab.
        let last = self.chat_tab_count().saturating_sub(1);
        match key.bare_key {
            BareKey::Left => self.chat_tab_focus = Some(selected.saturating_sub(1)),
            BareKey::Tab if Self::is_bar_key(&key) => {
                self.chat_tab_focus = Some(selected.saturating_sub(1))
            },
            BareKey::Right | BareKey::Tab => self.chat_tab_focus = Some((selected + 1).min(last)),
            BareKey::Enter => {
                self.chat_tab_focus = None;
                self.switch_to_tab(selected);
            },
            BareKey::Esc => self.chat_tab_focus = None,
            BareKey::Char('x') => {
                self.close_tab(selected);
                let last = self.chat_tab_count().saturating_sub(1);
                self.chat_tab_focus = Some(selected.min(last));
            },
            _ => {},
        }
        true
    }

    /// Tab on a half-typed `/name` or `@agent`. One match completes it, several
    /// are listed, and anything else is left alone — Tab is not a key the input
    /// box has any other use for.
    pub(crate) fn complete_tool_name(&mut self) -> bool {
        let text = self.input.get_text().trim().to_owned();
        let Some(prefix) = text.strip_prefix('/').filter(|rest| !rest.contains(' ')) else {
            return self.complete_mention();
        };
        let matches: Vec<String> = LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .chain(tools::names())
            .chain(self.extensions.names())
            .chain(self.skills.names())
            .filter(|name| name.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [name] => self.input.set_text(format!("/{name} ")),
            [] => return false,
            _ => {
                let notice = format!("/{}", matches.join("  /"));
                self.list_completions(notice);
            },
        }
        true
    }

    /// Tab with the cursor at the end of an `@token`, anywhere in the message.
    /// Candidates are the agents on screen right now; a bare `@` lists them all.
    pub(crate) fn complete_mention(&mut self) -> bool {
        let text = self.input.get_text().to_owned();
        let cursor = self.input.get_cursor_position();
        let head: String = text.chars().take(cursor).collect();
        let token = head.rsplit(char::is_whitespace).next().unwrap_or("");
        let Some(prefix) = token.strip_prefix('@') else {
            return false;
        };
        let matches: Vec<String> = agents::mention_candidates(&self.panes, &self.manual_kinds)
            .into_iter()
            .filter(|candidate| candidate.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [] => false,
            [only] => {
                // `token` is a suffix slice of `head`, so byte arithmetic is safe;
                // the cursor wants chars.
                let stem = &head[..head.len() - token.len()];
                let spliced = format!("{stem}@{only} ");
                let cursor_chars = spliced.chars().count();
                let tail: String = text.chars().skip(cursor).collect();
                self.input.set_text(format!("{spliced}{tail}"));
                self.input.set_cursor_position(cursor_chars);
                true
            },
            _ => {
                self.list_completions(format!("@{}", matches.join("  @")));
                true
            },
        }
    }

    /// The several-matches answer to Tab: the candidates as a notice line.
    pub(crate) fn list_completions(&mut self, notice: String) {
        self.messages.push(Message::notice(notice.clone()));
        self.log(
            "notice",
            serde_json::json!({"content": log::truncate(&notice)}),
        );
        self.pin_to_bottom();
    }

    pub(crate) fn on_mouse(&mut self, mouse: Mouse) -> bool {
        match mouse {
            Mouse::ScrollUp(lines) => self.scroll_up(lines.max(1)),
            Mouse::ScrollDown(lines) => self.scroll_down(lines.max(1)),
            Mouse::LeftClick(0, column) => {
                let target = ui::chat_tabs(self, self.chat_tab_width)
                    .into_iter()
                    .find(|cell| column >= cell.x && column < cell.x + cell.width)
                    .map(|cell| cell.target);
                match target {
                    Some(ui::TabTarget::Tab(tab)) => {
                        self.chat_tab_focus = None;
                        self.chat_tab_view.remove(&self.client);
                        self.switch_to_tab(tab);
                        true
                    },
                    Some(ui::TabTarget::Reveal(tab)) => {
                        self.chat_tab_focus = None;
                        self.chat_tab_view.insert(self.client, tab);
                        true
                    },
                    None => false,
                }
            },
            _ => false,
        }
    }

    pub(crate) fn submit(&mut self) -> bool {
        // Ahead of every guard, including the config: a slash command drives the
        // plugin rather than the conversation, so neither an API key nor a
        // settled reply is any of its business. `run_manual` turns away the one
        // kind that does conflict — another tool already running.
        if self.input.get_text().trim_start().starts_with('/') {
            let line = self.drain_input();
            return self.run_manual(&line);
        }
        // Typing at a plan is how you correct one, so whatever comes next is the
        // answer to the question and the question stops being asked. *Below* the
        // slash check: `/execute` is the approval and has to find the plan still
        // waiting, and `/plan` sets the mode for itself either way.
        if self.awaiting_plan() {
            // An empty Enter is not a correction — leave the prompt up rather
            // than silently dropping the approval with nothing recorded.
            if self.input.get_text().trim().is_empty() {
                self.drain_input();
                return true;
            }
            self.plan = Plan::Planning;
        }
        let text = self.drain_input();
        self.send_user_text(text)
    }

    /// Everything `submit` does once the text is known to be a message rather
    /// than a command: queue it behind an arriving reply, or start a request for
    /// it. Shared with `/plan <prompt>`, which is a mode switch and a question in
    /// one and so has to reach exactly this path.
    ///
    /// The text has already left the input box, so the two branches that refuse
    /// to send it put it back rather than swallow what was typed.
    pub(crate) fn send_user_text(&mut self, text: String) -> bool {
        self.send_user_message(text, None)
    }

    /// `send_user_text` for a turn that is shown as something other than what
    /// is sent. A skill arrives here with `content` as its expanded prompt and
    /// `display` as the `/name args` line that was typed: the typed line is
    /// what the transcript, the queue notice and a refusal's put-back show,
    /// and the prompt is what the model reads.
    pub(crate) fn send_user_message(&mut self, mut text: String, display: Option<String>) -> bool {
        // Cloned rather than borrowed so the transcript can be mutated below.
        let Ok(cfg) = self.cfg.clone() else {
            self.input.set_text(display.unwrap_or(text));
            return false;
        };

        // One stream slot, but no reason to throw away what the user typed.
        // The oldest queued question joins the exchange after its next complete
        // tool batch, or starts after a plain reply settles.
        if self.is_in_flight() {
            if self.asks.len() >= MAX_QUEUED_ASKS {
                let notice = "the question queue is full — the input was not sent";
                self.messages.push(Message::notice(notice));
                self.log("notice", serde_json::json!({"content": notice}));
                self.input.set_text(display.unwrap_or(text));
                self.pin_to_bottom();
                return true;
            }
            if text.trim().is_empty() {
                return true;
            }
            let ask = Ask {
                content: text,
                display,
            };
            let notice = format!("queued: {}", ask.shown());
            self.messages.push(Message::notice(notice.clone()));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(&notice)}),
            );
            self.asks.push_back(ask);
            self.pin_to_bottom();
            self.arm_timer(FRAME_SECONDS);
            return true;
        }

        if text.trim().is_empty() {
            // Whitespace-only: the input was still drained, so redraw.
            return true;
        }
        // An @-mention earns the model a context line naming the agent pane, so
        // "ask @codex to run the tests" needs no agent_list round first.
        if let Some(context) = agents::mention_context(
            &text,
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.hooks,
        ) {
            text.push_str("\n\n");
            text.push_str(&context);
        }

        let content = text.clone();
        self.scheduled_run = None;
        let typed = display.clone();
        self.messages.push(Message::user_shown(text, display));
        self.log(
            "user_input",
            serde_json::json!({"content": log::truncate(&content), "display": typed}),
        );
        // A new question starts a fresh tool budget.
        self.tool_rounds = 0;
        self.start_request(&cfg);
        true
    }

    pub(crate) fn drain_input(&mut self) -> String {
        let old_len = self.input.history().len();
        let text = self.input.drain_text();
        if self.input.history().len() != old_len {
            // One row, not the whole history: this used to re-serialize the
            // entire vector and rewrite the file on every submit.
            if let Some(line) = self.input.history().last().cloned() {
                history::add_prompt(&self.private_store(), &line);
            }
        }
        text
    }

    /// Send the transcript as it currently stands. Used both for a new user
    /// message and for the follow-up after tool results, which differ only in
    /// what was appended before the call.
    pub(crate) fn start_request(&mut self, cfg: &Config) {
        // The last round of the budget goes out without tools, so the model has
        // to answer from what it already collected instead of dead-ending on the
        // cap with nothing to show the user.
        let allow_tools = self.tool_rounds < self.max_tool_rounds();
        if !allow_tools {
            self.messages.push(Message::notice(
                "tool budget spent — answering without tools",
            ));
        }
        // The placeholder streamed text appends into. request_body skips empty
        // assistant messages, so it never reaches the wire.
        self.reply_at = Some(self.messages.len());
        self.messages.push(Message::assistant(""));

        self.stream_seq += 1;
        let stream_id = format!("hocus-{}", self.stream_seq);
        self.sse = SseParser::default();
        self.tool_calls = ToolCallAccumulator::default();

        let project = self
            .active_project
            .as_deref()
            .and_then(|name| self.projects.get(name))
            .map(|project| (project.name.as_str(), project.root.as_str()));
        // A request going out is the answer to any plan waiting on one — from
        // `approve_plan`, from something typed, from a queued ask or from a job
        // coming due. Clearing it in the one place they all pass through keeps
        // "a request is in flight" and "waiting on an approval" from ever being
        // true at once.
        if self.awaiting_plan() {
            self.plan = Plan::Planning;
        }
        let extension_specs = self.extensions.specs(self.is_planning());
        let body = api::request_body_with_project_and_tools(
            cfg,
            &self.messages,
            allow_tools,
            project,
            self.is_planning(),
            &extension_specs,
        );
        let body_len = body.len();
        self.log(
            "api_request",
            serde_json::json!({
                "model": cfg.model,
                "base_url": cfg.base_url,
                "stream_id": stream_id,
                "message_count": self.messages.len(),
                "body_bytes": body_len,
                "allow_tools": allow_tools,
            }),
        );

        web_request_stream(
            &stream_id,
            &cfg.url,
            HttpVerb::Post,
            api::request_headers(cfg),
            body,
            // Round-trips through the host, like the `models` tag. The head of a
            // stream is a `WebRequestResult` carrying no stream id of its own,
            // so without this there is no telling which chat a non-2xx belongs
            // to once more than one is in flight.
            BTreeMap::from([(STREAM_CONTEXT.to_owned(), stream_id.clone())]),
            None,
        );

        self.stream = Some(StreamState {
            id: stream_id,
            finishing: false,
            usage: None,
        });
        self.pin_to_bottom();
        // After the pin, which clears it: everyone starts this reply following
        // the tail, and is held at its first line once it outgrows the pane.
        self.view.anchor = self.reply_at;
        self.arm_timer(FRAME_SECONDS);
    }

    /// The single `WebRequestResult` that leads every stream — and, when the
    /// context says so, the whole answer to a `web_search` or a model list.
    pub(crate) fn on_response(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if self.finish_search(status, body, context) {
            return true;
        }
        if self.finish_linear(status, body, context) {
            return true;
        }
        if self.finish_models(status, body, context) {
            return true;
        }
        // Someone else's stream, or one we already cancelled and forgot.
        let Some(stream_id) = context.get(STREAM_CONTEXT).cloned() else {
            return false;
        };
        if self.active_stream_id() != Some(stream_id.clone()) {
            return false;
        }
        if (200..300).contains(&status) {
            // A 2xx carries only status and headers; the body arrives as chunks.
            return false;
        }

        // Non-2xx: the entire error body is here and no chunks will follow.
        self.pop_empty_placeholder();
        let err = api::api_error_message(status, body);
        self.messages.push(Message::error(err.clone()));
        self.log(
            "api_error",
            serde_json::json!({
                "stream_id": stream_id.clone(),
                "status": status,
                "error": log::truncate(&err),
            }),
        );
        self.scheduled_run = None;
        self.stream = Some(StreamState {
            id: stream_id,
            finishing: true,
            usage: None,
        });
        self.pin_to_bottom();
        true
    }

    pub(crate) fn on_chunk(
        &mut self,
        stream_id: &str,
        chunk: &[u8],
        done: bool,
        error: Option<String>,
    ) -> bool {
        // A stream we superseded, cancelled and forgot, or never started.
        if self.active_stream_id().as_deref() != Some(stream_id) {
            return false;
        }
        if !self.is_receiving() {
            if done {
                self.stream = None;
                self.reply_at = None;
                self.tool_calls.take();
                self.scheduled_run = None;
            }
            return done;
        }

        let items = self.sse.feed(chunk);
        for item in items {
            match item {
                SseItem::Delta(text) => {
                    self.append_to_reply(&text);
                },
                SseItem::Reasoning { text, key } => {
                    self.append_to_reasoning(&text, key);
                },
                SseItem::ApiError(message) => {
                    let m = message.clone();
                    self.pop_empty_placeholder();
                    self.messages.push(Message::error(message));
                    self.log(
                        "api_error",
                        serde_json::json!({
                            "stream_id": stream_id,
                            "error": log::truncate(&m),
                        }),
                    );
                    self.tool_calls.take();
                    self.scheduled_run = None;
                    self.stream = Some(StreamState {
                        id: stream_id.to_owned(),
                        finishing: true,
                        usage: None,
                    });
                    break;
                },
                SseItem::ToolCallFragment { .. } => self.tool_calls.feed(&item),
                // Held on the stream rather than recorded here: Ollama hangs
                // the count on the last content chunk, ahead of the text it
                // covers, and a tool-calling reply is still fragments in
                // `tool_calls`. The baseline is taken at `done`, once the
                // transcript holds what was counted.
                SseItem::Usage { prompt, completion } => {
                    if let Some(stream) = self.stream.as_mut() {
                        stream.usage = Some(prompt + completion);
                    }
                    self.log(
                        "api_usage",
                        serde_json::json!({
                            "stream_id": stream_id,
                            "prompt_tokens": prompt,
                            "completion_tokens": completion,
                        }),
                    );
                },
                // Informational: `done: true` below is the authoritative end.
                SseItem::Done => {},
            }
        }

        if done {
            let settled = self.stream.take();
            // A cancelled or already-failed stream's count is not recorded
            // even if one arrived: the outcome on screen is not what it cost.
            let usage = settled
                .as_ref()
                .filter(|stream| !stream.finishing)
                .and_then(|stream| stream.usage);
            match settled {
                // Outcome already recorded; this is the terminator we were
                // waiting to swallow. Any half-accumulated calls go with it.
                Some(stream) if stream.finishing => {
                    self.tool_calls.take();
                },
                _ => {
                    let calls = self.tool_calls.take();
                    if let Some(message) = error {
                        let m = message.clone();
                        self.pop_empty_placeholder();
                        self.messages.push(Message::error(message));
                        self.log(
                            "api_error",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "error": log::truncate(&m),
                            }),
                        );
                    } else if !calls.is_empty() {
                        // DeepSeek requires the prose, reasoning and tool calls
                        // from one response to remain one assistant message.
                        let (prior_text, prior_reasoning) = self
                            .reply()
                            .map(|message| {
                                (
                                    std::mem::take(&mut message.content),
                                    message.reasoning.take(),
                                )
                            })
                            .unwrap_or_default();
                        self.pop_empty_placeholder();
                        let mut asked = Message::tool_calls(calls.clone());
                        if !prior_text.is_empty() {
                            asked.display = Some(format!(
                                "{prior_text}\n\n{}",
                                asked.display.take().unwrap_or_default()
                            ));
                        }
                        asked.content = prior_text.clone();
                        asked.reasoning = prior_reasoning;
                        self.messages.push(asked);
                        // The swap moves neither the message count nor the
                        // length of the prose, which is all the context memo
                        // watches, so it has to be dropped by hand. The
                        // transcript's per-message key would in fact notice
                        // this one (the call summary arrives as `display`), but
                        // the invalidation is explicit so that guarantee does
                        // not rest on the coincidence.
                        let swapped = self.messages.len() - 1;
                        if let Some(cache) = self.transcript_cache.as_mut() {
                            cache.invalidate_from(swapped);
                        }
                        self.context_cache = None;
                        self.saved.1 = self.mutable_len().wrapping_add(1);
                        let tool_payload: Vec<serde_json::Value> = calls
                            .iter()
                            .map(|c| {
                                serde_json::json!({
                                    "id": c.id,
                                    "name": c.name,
                                    "arguments": log::truncate(&c.arguments),
                                })
                            })
                            .collect();
                        self.log(
                            "tool_call",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "tool_calls": tool_payload,
                            }),
                        );
                        if !prior_text.is_empty() {
                            self.log(
                                "assistant_message",
                                serde_json::json!({
                                    "stream_id": stream_id,
                                    "content": log::truncate(&prior_text),
                                }),
                            );
                        }
                        // Extended, not replaced: a tool the user typed
                        // mid-stream may still be running, and its result is
                        // owed to the transcript before these go out. Pumping is
                        // then that call's job — starting a second one here
                        // would run two tools at once.
                        //
                        // ponytail: messages land in arrival order, so a typed
                        // call still running when these arrive has its result
                        // recorded *after* this summary — each result still
                        // follows the call that asked for it, but the two pairs
                        // interleave. Endpoints that want a result immediately
                        // after its call would need the transcript reordered on
                        // the way out in `api::request_body`.
                        self.pending.extend(calls);
                        self.owes_reply = true;
                        if !self.tools_busy() {
                            self.pump_tools();
                        }
                    } else if self.reply_is_empty() {
                        self.pop_empty_placeholder();
                        self.messages
                            .push(Message::error("empty response from endpoint"));
                        self.log(
                            "api_error",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "error": "empty response from endpoint",
                            }),
                        );
                    } else {
                        // Successful assistant reply.
                        let reply = self
                            .reply()
                            .map(|message| message.content.clone())
                            .unwrap_or_default();
                        self.log(
                            "assistant_message",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "content": log::truncate(&reply),
                            }),
                        );
                        self.log(
                            "api_response",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "status": "done",
                            }),
                        );
                        // A planning turn that ended in prose is a plan. One
                        // that ended in tool calls went down the branch above
                        // and is still researching; an error or an empty
                        // response leaves the mode alone to be retried.
                        if self.is_planning() {
                            self.plan_cache = None;
                            self.plan = Plan::AwaitingApproval;
                            self.pin_to_bottom();
                            // The sidebar is very often not what is being looked
                            // at — the same reason `pump_tools` toasts its own
                            // y/n rather than trusting the pane to be on screen.
                            notify(
                                "hocus has a plan — /execute to carry it out, /execute --clear \
                                 to carry it out in a fresh chat",
                            );
                        }
                    }
                },
            }
            // The transcript now holds everything the provider counted — the
            // prose, or the tool-calls message with its arguments — so this is
            // the baseline that "added since" is measured against. Only a tool
            // result, or the next question, moves the count off exact.
            if let Some(tokens) = usage {
                self.usage = Some(ContextUsage {
                    tokens,
                    wire_chars: api::wire_chars(&self.messages),
                });
                self.context_cache = None;
            }
            // The reply is settled, so nothing is appending any more — whatever
            // a typed tool pushes from here on is its own message.
            self.reply_at = None;
            if !self.is_in_flight() {
                self.scheduled_run = None;
            }
        }
        // Only the terminal chunk renders. Mid-stream chunks arrive once per
        // socket read (~50/s), and every plugin render costs the host a full
        // screen composite on the same thread that services every other Zellij
        // action — rendering per chunk freezes the whole session for the length
        // of the reply. The spinner timer repaints the arriving text instead.
        done
    }

    /// The message for a Slack tool whose `dobbs` could not be executed, or
    /// `None` when this is any other kind of result.
    ///
    /// Matched on the stderr string rather than the code alone: Zellij uses
    /// exit code 2 both for a failure to spawn and, unavoidably, for a command
    /// that ran and exited 2 by itself.
    pub(crate) fn slack_exec_hint(&self, exit_code: Option<i32>, stderr: &[u8]) -> Option<String> {
        let head = self.pending.front()?;
        if !tools::SLACK_TOOLS.contains(&head.name.as_str()) || exit_code != Some(2) {
            return None;
        }
        let stderr = String::from_utf8_lossy(stderr);
        if !stderr.contains("os error 2") {
            return None;
        }
        let dobbs = self.cfg.as_ref().ok().map(|cfg| cfg.dobbs.as_str())?;
        Some(format!(
            "dobbs could not be run: \"{dobbs}\" does not exist or is not executable.\n\n\
             Set dobbs_path in the hocus plugin block of ~/.config/zellij/config.kdl to the \
             binary's absolute path — a leading ~/ is expanded, but nothing else is. Tell the \
             user this; do not try other commands to find dobbs yourself."
        ))
    }

    /// The message for a file tool whose binary could not be executed, or `None`
    /// when this is any other kind of result.
    ///
    /// Same shape as `slack_exec_hint` and for the same reason: the model never
    /// sees the argv, so "No such file or directory" names nothing it can act
    /// on. Unlike dobbs there is no setting to point at — `rg` and `sed` are
    /// looked up on PATH — so the only useful thing to say is which one is
    /// missing and that hunting for it is not on the table.
    fn project_exec_hint(&self, exit_code: Option<i32>, stderr: &[u8]) -> Option<String> {
        let head = self.pending.front()?;
        let is_project = tools::PROJECT_READ_TOOLS.contains(&head.name.as_str());
        if !is_project || exit_code != Some(2) {
            return None;
        }
        if !String::from_utf8_lossy(stderr).contains("os error 2") {
            return None;
        }
        let binary = match head.name.as_str() {
            "read_file" => "sed",
            "find_files" | "search_files" => "rg",
            "git_log" | "git_show" => "git",
            "github_read" => "gh",
            _ => return None,
        };
        let install = if binary == "rg" {
            " — ripgrep provides rg"
        } else {
            ""
        };
        Some(format!(
            "{binary} could not be run: it is not installed or not on PATH{install}.\n\n\
             Tell the user this; do not try other commands, and do not ask a pane to run one \
             on your behalf."
        ))
    }

    /// What a file tool's command becomes for the model.
    ///
    /// `rg` exits 1 when it ran perfectly well and simply matched nothing, which
    /// as a bare "exit code: 1" reads as a broken tool and invites the model to
    /// retry it. Say what actually happened instead.
    ///
    /// The listing is cut to the call's line cap before `MAX_TOOL_OUTPUT` gets
    /// near it, so a search of a large repository comes back as a usable head of
    /// the matches with a count, rather than 40 000 bytes ending mid-line.
    /// The reload proper, once the host has said the file is there to be loaded.
    ///
    /// `start_or_reload_plugin` returns nothing, so this cannot answer yet: it asks, records
    /// what it asked for, and leaves the head pending for `poll_plugin_load` to answer once the
    /// pane has had time to show what happened. The path is re-resolved rather than carried:
    /// `resolve_plugin_file` is pure and was already given the same folder and name, so it
    /// cannot answer differently here, and a reload must be built from an absolute path -
    /// `RunPluginLocation::parse` joins a relative `file:` path onto the cwd, which
    /// `start_or_reload_plugin` always supplies.
    pub(crate) fn start_plugin_load(&mut self) -> bool {
        let Some(head) = self.pending.front() else {
            return false;
        };
        let target = tools::Args::parse(&head.arguments).ok().and_then(|args| {
            tools::resolve_plugin_file(&self.user_plugin_dir, args.name.as_deref()).ok()
        });
        let Some(target) = target else {
            self.finish_head("the plugin could not be resolved to a file");
            self.pump_tools();
            return true;
        };
        let extension =
            extensions::namespace_for_path(&self.user_plugin_dir, &target).map(str::to_owned);
        if let Some(namespace) = &extension {
            self.extension_files
                .insert(namespace.clone(), target.clone());
            if let Some(owner) = self.extensions.owner_for_path(&target) {
                self.extensions.remove_owner(owner);
                extensions::reload(owner);
            } else {
                extensions::load(&target, namespace);
            }
        } else {
            tools::reload_plugin(&target);
        }
        self.plugin_load = Some(PluginLoadWatch {
            target,
            started_ms: self.mono_ms,
            extension: extension.is_some(),
        });
        self.status = Status::AwaitingPluginLoad;
        self.arm_timer(FRAME_SECONDS);
        true
    }

    fn project_tool_result(
        head: &ToolCall,
        exit_code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
    ) -> String {
        let stdout = String::from_utf8_lossy(stdout);
        let stdout = stdout.trim_end_matches('\n');
        let stderr = String::from_utf8_lossy(stderr);
        let stderr = stderr.trim();

        if tools::FILE_TOOLS.contains(&head.name.as_str())
            && exit_code == Some(1)
            && stdout.is_empty()
            && stderr.is_empty()
        {
            return "no matches".to_owned();
        }
        let successful = exit_code == Some(0)
            || tools::FILE_TOOLS.contains(&head.name.as_str()) && exit_code == Some(1);
        if !successful {
            let mut result = format!("exit code: {}", exit_code.unwrap_or(-1));
            if !stderr.is_empty() {
                result.push_str(&format!(
                    "\n\n{}",
                    log::truncate_str(stderr, MAX_TOOL_OUTPUT)
                ));
            }
            return result;
        }
        if stdout.is_empty() {
            return if head.name == "read_file" {
                // `sed` on an empty range, or a start_line past the end of the file.
                "no lines".to_owned()
            } else {
                "no output".to_owned()
            };
        }

        let cap = tools::FILE_TOOLS
            .contains(&head.name.as_str())
            .then(|| tools::Args::parse(&head.arguments).ok())
            .flatten()
            .and_then(|args| tools::file_line_cap(&head.name, &args));
        let total = stdout.lines().count();
        let mut result = match cap {
            Some(cap) if total > cap => {
                let kept: Vec<&str> = stdout.lines().take(cap).collect();
                format!(
                    "{}\n\n… {} of {total} lines shown; narrow the search or raise max_results.",
                    kept.join("\n"),
                    cap
                )
            },
            _ => stdout.to_owned(),
        };
        result = log::truncate_str(&result, MAX_TOOL_OUTPUT);
        // rg writes per-file complaints here — an unreadable file, a bad glob —
        // while still exiting 0 on what it did match. Worth passing on.
        if !stderr.is_empty() {
            result.push_str(&format!(
                "\n\nstderr:\n{}",
                log::truncate_str(stderr, MAX_TOOL_OUTPUT)
            ));
        }
        result
    }

    /// The deferred half of the Slack and file tools. The context map
    /// round-tripped through the host, so it names the call this answers.
    ///
    /// Each stream is capped at `MAX_TOOL_OUTPUT`: a tool result stays in the
    /// transcript, so it is re-sent and re-billed on every later turn of the
    /// conversation. One `slack_read` of a busy channel would otherwise make
    /// every subsequent message cost a full transcript.
    pub(crate) fn on_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !matches!(self.status, Status::AwaitingCommand) {
            return false;
        }
        // A result from a run we already aborted, or from someone else's call.
        let Some(head) = self.pending.front() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }

        // Zellij reports a command it could not even start as exit code 2 with
        // the raw io::Error as the whole of stderr — "No such file or directory
        // (os error 2)", naming nothing. For a Slack tool the model never sees
        // the argv, so that message is undiagnosable from inside the
        // conversation: say which binary, and which setting names it.
        let result = match self
            .slack_exec_hint(exit_code, stderr)
            .or_else(|| self.project_exec_hint(exit_code, stderr))
        {
            Some(hint) => hint,
            None if tools::PROJECT_READ_TOOLS.contains(&head.name.as_str()) => {
                Self::project_tool_result(head, exit_code, stdout, stderr)
            },
            None => {
                let mut result = format!("exit code: {}", exit_code.unwrap_or(-1));
                for (label, bytes) in [("stdout", stdout), ("stderr", stderr)] {
                    let text = String::from_utf8_lossy(bytes);
                    if !text.trim().is_empty() {
                        let text = log::truncate_str(text.trim_end(), MAX_TOOL_OUTPUT);
                        result.push_str(&format!("\n\n{label}:\n{text}"));
                    }
                }
                result
            },
        };

        self.finish_head(result);
        self.pump_tools();
        true
    }
}
