use agent_proto::{projects, wire::PollReply};
use std::{collections::BTreeMap, path::Path};
use yinz_tile::prelude::ProjectPaneGroup;

fn configuration_root(common_dir: Option<&str>, fallback: &str) -> String {
    common_dir
        .map(Path::new)
        .filter(|path| path.file_name().is_some_and(|name| name == ".git"))
        .and_then(Path::parent)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Exact probes only: borrowing an ancestor's answer can hide a nested worktree.
pub fn classify(
    registry: &projects::Registry,
    reply: &PollReply,
) -> (Vec<ProjectPaneGroup>, usize) {
    let mut groups: BTreeMap<String, (ProjectPaneGroup, Vec<_>)> = BTreeMap::new();
    let mut skipped = 0;
    for (position, pane) in reply.panes.iter().flatten().enumerate() {
        let Some(location) = pane.checkout_location() else {
            skipped += 1;
            continue;
        };
        let repository = reply.repositories.get(&location);
        let registered = projects::for_cwd(registry, &location)
            .filter(|project| !project.discovered)
            .or_else(|| repository.and_then(|repo| projects::for_repository(registry, repo)))
            .or_else(|| {
                repository
                    .filter(|repo| repo.is_checkout())
                    .and_then(|_| projects::for_cwd(registry, &location))
            });
        let identity = if let Some(project) = registered {
            Some((
                format!("project:{}", project.root),
                project.name.clone(),
                Some(configuration_root(
                    project.common_dir.as_deref(),
                    &project.root,
                )),
            ))
        } else {
            repository
                .filter(|repo| repo.is_checkout())
                .and_then(|repo| {
                    let identity = repo
                        .remote
                        .as_ref()
                        .filter(|remote| !remote.is_empty())
                        .map(|remote| format!("remote:{}", projects::origin_identity(remote)))
                        .or_else(|| {
                            repo.common_dir
                                .as_ref()
                                .map(|dir| format!("git:{:?}:{dir}", repo.host))
                        })?;
                    let root = repo
                        .host
                        .is_none()
                        .then(|| configuration_root(repo.common_dir.as_deref(), &repo.root));
                    Some((identity, repo.checkout_name()?, root))
                })
        };
        let Some((identity, name, configuration_root)) = identity else {
            skipped += 1;
            continue;
        };
        let (group, order) = groups.entry(identity.clone()).or_insert_with(|| {
            (
                ProjectPaneGroup {
                    identity,
                    name,
                    configuration_root: configuration_root.clone(),
                    pane_ids: vec![],
                },
                vec![],
            )
        });
        if group.configuration_root.is_none() {
            group.configuration_root = configuration_root;
        }
        order.push((
            repository.and_then(|repo| repo.branch.clone()),
            location.host,
            repository
                .map(|repo| repo.root.clone())
                .unwrap_or(location.path),
            position,
            pane.pane_id,
        ));
    }
    (
        groups
            .into_values()
            .map(|(mut group, mut order)| {
                order.sort();
                group.pane_ids = order.into_iter().map(|(_, _, _, _, id)| id).collect();
                group
            })
            .collect(),
        skipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::{agents, location::Location};
    use yinz_tile::prelude::TabId;

    fn pane(id: u32, cwd: &str) -> agents::AgentPane {
        agents::AgentPane {
            pane_id: id,
            title: String::new(),
            command: None,
            cwd: Some(cwd.into()),
            host: None,
            firecracker_image: None,
            firecracker_worktree: None,
            bubblewrap: None,
            status: agents::AgentStatus::Running,
            tab: Some((TabId::new(0), String::new())),
            detected: None,
            agent: None,
        }
    }
    fn reply(panes: Vec<agents::AgentPane>, repositories: projects::Probed) -> PollReply {
        PollReply {
            suppressed_panes: Vec::new(),
            panes: Some(panes),
            repositories,
            activity: agents::Activity::new(),
            manual_kinds: BTreeMap::new(),
            yinz_session_name: String::new().into(),
            tab_projects: BTreeMap::new(),
            focused: None,
            hooks: agents::HookFeeds::new(),
            summaries: agents::Summaries::new(),
            runs: vec![],
        }
    }
    fn repo(
        root: &str,
        common: &str,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> projects::Repository {
        projects::Repository {
            root: root.into(),
            common_dir: Some(common.into()),
            remote: remote.map(str::to_owned),
            branch: branch.map(str::to_owned),
            ..Default::default()
        }
    }
    #[test]
    fn https_and_ssh_checkouts_share_a_group_with_or_without_a_registered_project() {
        let snapshot = reply(
            vec![pane(1, "/first"), pane(2, "/second")],
            BTreeMap::from([
                (
                    Location::local("/first"),
                    repo(
                        "/first",
                        "/first/.git",
                        Some("https://example.com/owner/repo.git"),
                        None,
                    ),
                ),
                (
                    Location::local("/second"),
                    repo(
                        "/second",
                        "/second/.git",
                        Some("git@EXAMPLE.com:owner/repo"),
                        None,
                    ),
                ),
            ]),
        );
        let mut project = projects::validate("kept", "/elsewhere").unwrap();
        project.remote = Some("ssh://git@example.com:22/owner/repo.git/".into());
        for registry in [BTreeMap::new(), BTreeMap::from([("kept".into(), project)])] {
            let (groups, skipped) = classify(&registry, &snapshot);
            assert_eq!(skipped, 0);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].pane_ids, vec![1, 2]);
            if !registry.is_empty() {
                assert_eq!(groups[0].name, "kept");
            }
        }
    }

    #[test]
    fn identity_and_checkout_order_are_independent_of_display_names() {
        let mut remote = pane(3, "/process");
        remote.agent = Some(
            serde_json::from_value(
                serde_json::json!({"kind":"codex", "state":"Busy", "cwd":"/repo"}),
            )
            .unwrap(),
        );
        remote.host = Some("ssh-box".into());
        let mut vm = pane(4, "/guest");
        vm.host = Some("vm".into());
        vm.firecracker_worktree = Some("/repo/.worktrees/a".into());
        let mut agent = pane(5, "/shell");
        agent.agent = Some(
            serde_json::from_value(
                serde_json::json!({"kind":"codex", "state":"Busy", "cwd":"/repo/.worktrees/a"}),
            )
            .unwrap(),
        );
        let panes = vec![pane(1, "/repo"), pane(2, "/other/repo"), remote, vm, agent];
        let repositories = BTreeMap::from([
            (
                Location::local("/repo"),
                repo("/repo", "/repo/.git", Some("origin-a"), Some("z")),
            ),
            (
                Location::local("/other/repo"),
                repo("/other/repo", "/other/repo/.git", Some("origin-b"), None),
            ),
            (
                Location::new(Some("ssh-box"), "/repo"),
                repo("/repo", "/repo/.git", Some("origin-a"), Some("b"))
                    .on_host(Some("ssh-box".into())),
            ),
            (
                Location::local("/repo/.worktrees/a"),
                repo(
                    "/repo/.worktrees/a",
                    "/repo/.git",
                    Some("origin-a"),
                    Some("a"),
                ),
            ),
        ]);
        let (groups, skipped) = classify(&BTreeMap::new(), &reply(panes, repositories));
        assert_eq!(skipped, 0);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, groups[1].name);
        assert_ne!(groups[0].identity, groups[1].identity);
        assert_eq!(groups[0].pane_ids, vec![4, 5, 3, 1]);
        assert_eq!(groups[0].configuration_root.as_deref(), Some("/repo"));
    }
    #[test]
    fn remote_less_paths_are_host_scoped_and_explicit_folders_are_recognized() {
        let mut remote = pane(2, "/repo");
        remote.host = Some("other".into());
        let registered = projects::validate("notes", "/notes").unwrap();
        let repositories = BTreeMap::from([
            (
                Location::local("/repo"),
                repo("/repo", "/repo/.git", None, None),
            ),
            (
                Location::new(Some("other"), "/repo"),
                repo("/repo", "/repo/.git", None, None).on_host(Some("other".into())),
            ),
        ]);
        let (groups, skipped) = classify(
            &BTreeMap::from([("notes".into(), registered)]),
            &reply(
                vec![
                    pane(1, "/repo"),
                    remote,
                    pane(3, "/notes/subdir"),
                    pane(4, "/unknown"),
                ],
                repositories,
            ),
        );
        assert_eq!(groups.len(), 3);
        assert_eq!(skipped, 1);
        assert_eq!(
            groups
                .iter()
                .filter(|g| g.configuration_root.is_none())
                .count(),
            1
        );
    }
    #[test]
    fn a_missing_nested_probe_does_not_borrow_the_primary_branch() {
        let snapshot = reply(
            vec![pane(1, "/repo/.worktrees/topic")],
            BTreeMap::from([(
                Location::local("/repo"),
                repo("/repo", "/repo/.git", None, Some("main")),
            )]),
        );
        assert_eq!(classify(&BTreeMap::new(), &snapshot), (vec![], 1));
    }
}
