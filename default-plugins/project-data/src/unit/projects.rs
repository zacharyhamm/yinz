use super::*;

#[test]
fn origin_identity_matches_transports_but_preserves_repository_paths_and_ports() {
    let canonical = origin_identity("https://example.com/Owner/Repo");
    for origin in [
        "https://EXAMPLE.com:443/Owner/Repo.git/",
        "HTTPS://EXAMPLE.COM/Owner/Repo/",
        "git@EXAMPLE.com:Owner/Repo.git",
        "ssh://git@EXAMPLE.com:22/Owner/Repo.git/",
        "ssh://git@example.com/Owner/Repo",
    ] {
        assert_eq!(origin_identity(origin), canonical, "{origin}");
    }
    for origin in [
        "https://other.com/Owner/Repo",
        "https://example.com/Other/Repo",
        "https://example.com/owner/Repo",
        "https://example.com/Owner/repo",
        "https://example.com:8443/Owner/Repo",
        "ssh://git@example.com:2222/Owner/Repo",
        "ssh://git@example.com:443/Owner/Repo",
        "https://example.com:22/Owner/Repo",
    ] {
        assert_ne!(origin_identity(origin), canonical, "{origin}");
    }
    assert_eq!(
        origin_identity("https://example.com:8443/Owner/Repo.git"),
        origin_identity("ssh://git@example.com:8443/Owner/Repo")
    );
    for origin in [
        "",
        "/Owner/Repo.git",
        "../Repo.git",
        "example.com/Owner/Repo",
        "file:///Owner/Repo.git",
        "http://example.com/Owner/Repo",
        "host:Owner/Repo",
        "https:///Owner/Repo",
        "https://",
        "ssh://git@/Owner/Repo",
        "https://example.com:/Owner/Repo",
        "https://example.com:bad/Owner/Repo",
        "https://example.com:65536/Owner/Repo",
        "https://example.com/",
        "https://example.com/.git",
        "https://example.com/Owner/../Repo",
        "https://example.com/Owner/%2e%2e/Repo",
        "https://example.com/Owner//Repo",
        "https://example.com/Owner/Repo?query",
        "https://example.com/Owner/Repo#fragment",
        " https://example.com/Owner/Repo",
        "https://example.com/Owner/Repo\n",
        "git@example.com:/Owner/Repo",
        "git@/local:Owner/Repo",
    ] {
        assert_eq!(
            origin_identity(origin),
            format!("exact:{origin}"),
            "{origin}"
        );
    }
    assert_ne!(
        origin_identity("/Owner/Repo.git"),
        origin_identity("/Owner/Repo")
    );
    assert!(equivalent_origins(
        Some("custom:exact"),
        Some("custom:exact")
    ));
    assert!(!equivalent_origins(Some(""), Some("")));
    assert!(!equivalent_origins(None, None));
}

fn project_rows(backend: &impl Backend) -> DataRows {
    backend
        .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
        .unwrap()
}

fn merge_projects(backend: &impl Backend) -> (Project, Project) {
    let mut source = validated("duplicate", "/source");
    source.remote = Some("git@example.com:Owner/Repo.git".into());
    source.worktree_dir = Some("/source-worktrees".into());
    let mut target = validated("kept", "/target");
    target.remote = Some("https://example.com/Owner/Repo".into());
    target.worktree_dir = Some("../target-worktrees".into());
    target.common_dir = Some("/target/.git".into());
    target.worktree_status = Some(WorktreeStatus::Linked);
    target.discovered = true;
    backend
        .apply(vec![to_mutation(&source), to_mutation(&target)])
        .unwrap();
    (source, target)
}

#[test]
fn replacement_history_survives_renames_merges_and_updates_but_not_name_reuse() {
    let backend = backend();
    merge_projects(&backend);
    let first = row_ids(&project_rows(&backend))["duplicate"];
    backend
        .apply(
            plan_rename(&project_rows(&backend), "duplicate", "renamed")
                .unwrap()
                .mutations,
        )
        .unwrap();
    let renamed = row_ids(&project_rows(&backend))["renamed"];
    backend
        .apply(
            plan_merge(&project_rows(&backend), "renamed", "kept")
                .unwrap()
                .mutations,
        )
        .unwrap();
    let kept = row_ids(&project_rows(&backend))["kept"];
    backend
        .apply(
            plan_rename(&project_rows(&backend), "kept", "last")
                .unwrap()
                .mutations,
        )
        .unwrap();
    let expected = vec![first, kept, renamed];
    let mut expected = expected;
    expected.sort_unstable();
    assert_eq!(read_back(&backend)["last"].replaced_ids, expected);
    let (adopted, mutations) =
        plan_adopt(&project_rows(&backend), validated("last", "/changed")).unwrap();
    backend.apply(mutations).unwrap();
    assert_eq!(adopted.project.replaced_ids, expected);
    assert_eq!(read_back(&backend)["last"].replaced_ids, expected);
    backend.apply(vec![delete_mutation("last")]).unwrap();
    let (_, mutations) =
        plan_adopt(&project_rows(&backend), validated("last", "/changed")).unwrap();
    backend.apply(mutations).unwrap();
    assert!(read_back(&backend)["last"].replaced_ids.is_empty());
}

#[test]
fn adoption_records_replacements_and_conflicts_publish_nothing() {
    for conflict in [false, true] {
        let backend = backend();
        let mut found = validated("found", "/work");
        found.discovered = true;
        found.replaced_ids = vec![DataRowId::new(900)];
        backend.apply(vec![to_mutation(&found)]).unwrap();
        let rows = project_rows(&backend);
        let id = row_ids(&rows)["found"];
        let (adopted, mutations) = plan_adopt(&rows, validated("mine", "/work")).unwrap();
        assert_eq!(adopted.project.replaced_ids, vec![id, DataRowId::new(900)]);
        if conflict {
            backend
                .apply(vec![Mutation::update("project", Pred::eq("name", "found"))
                    .set("worktree_dir", "changed")
                    .build()])
                .unwrap();
            let before = project_rows(&backend);
            assert!(backend.apply(mutations).unwrap_err().is_conflict());
            assert_eq!(project_rows(&backend), before);
        } else {
            backend.apply(mutations).unwrap();
            assert_eq!(read_back(&backend)["mine"], adopted.project);
        }
    }
}

#[test]
fn rename_validates_names_and_preserves_all_metadata() {
    let backend = backend();
    let (_, mut expected) = merge_projects(&backend);
    let rows = project_rows(&backend);
    for name in ["", " \t ", "bad\nname", "duplicate", "kept"] {
        assert!(plan_rename(&rows, "kept", name).is_err(), "{name:?}");
    }
    assert!(plan_rename(&rows, "missing", "new").is_err());
    let change = plan_rename(&rows, "kept", "  new name  ").unwrap();
    expected.replaced_ids = vec![row_ids(&rows)["kept"]];
    expected.name = "new name".into();
    expected.discovered = false;
    assert_eq!(change.project, expected);
    backend.apply(change.mutations).unwrap();
    let registry = read_back(&backend);
    assert!(!registry.contains_key("kept"));
    assert_eq!(registry["new name"], expected);
}

#[test]
fn merge_preserves_target_settings() {
    let backend = backend();
    let (source, mut expected) = merge_projects(&backend);
    let rows = project_rows(&backend);
    let change = plan_merge(&rows, "duplicate", "kept").unwrap();
    expected.replaced_ids = vec![row_ids(&rows)["duplicate"]];
    expected.discovered = false;
    assert_eq!(change.project, expected);
    backend.apply(change.mutations).unwrap();
    let mut registry = read_back(&backend);
    assert_eq!(
        registry,
        Registry::from([("kept".into(), expected.clone())])
    );
    // Even a primary source checkout cannot move the user-managed target or recreate it.
    let source_checkout = git(
        &source.root,
        source.remote.as_deref(),
        WorktreeStatus::Primary,
    );
    assert!(!discover(&mut registry, std::slice::from_ref(&source_checkout)).changed());
    assert_eq!(for_repository(&registry, &source_checkout), Some(&expected));
    assert_eq!(registry.len(), 1);
}

#[test]
fn merge_requires_distinct_matching_origins() {
    let backend = backend();
    let (source, mut target) = merge_projects(&backend);
    let rows = project_rows(&backend);
    assert!(plan_merge(&rows, "kept", "kept").is_err());
    assert!(plan_merge(&rows, "missing", "kept").is_err());
    assert!(plan_merge(&rows, "duplicate", "missing").is_err());
    for remote in [None, Some(""), Some("ssh://git@elsewhere/Owner/Repo")] {
        target.remote = remote.map(str::to_owned);
        backend.apply(vec![to_mutation(&target)]).unwrap();
        assert!(plan_merge(&project_rows(&backend), "duplicate", "kept").is_err());
    }
    target.remote = source.remote.clone();
    backend
        .apply(vec![
            to_mutation(&target),
            to_mutation(&validated("shared", &source.root)),
        ])
        .unwrap();
    backend
        .apply(
            plan_merge(&project_rows(&backend), "duplicate", "kept")
                .unwrap()
                .mutations,
        )
        .unwrap();
    assert!(read_back(&backend).contains_key("shared"));
}

#[test]
fn project_changes_reject_stale_rows_and_new_names_atomically() {
    for conflict in ["source", "target", "destination", "deleted source"] {
        let backend = backend();
        merge_projects(&backend);
        let rows = project_rows(&backend);
        let change = if conflict == "destination" {
            plan_rename(&rows, "duplicate", "new").unwrap()
        } else {
            plan_merge(&rows, "duplicate", "kept").unwrap()
        };
        let concurrent = match conflict {
            "source" | "target" => Mutation::update(
                "project",
                Pred::eq(
                    "name",
                    if conflict == "source" {
                        "duplicate"
                    } else {
                        "kept"
                    },
                ),
            )
            .set("worktree_dir", "concurrent")
            .build(),
            "destination" => to_mutation(&validated("new", "/unrelated")),
            _ => delete_mutation("duplicate"),
        };
        backend.apply(vec![concurrent]).unwrap();
        let before = project_rows(&backend);
        // A preceding write is rolled back too: assertions are inside the transaction.
        let mut mutations = vec![to_mutation(&validated("rolled-back", "/rollback"))];
        mutations.extend(change.mutations);
        assert!(
            backend.apply(mutations).unwrap_err().is_conflict(),
            "{conflict}"
        );
        assert_eq!(project_rows(&backend), before, "{conflict}");
    }
    let backend = backend();
    merge_projects(&backend);
    let mut rows = project_rows(&backend);
    rows.truncated = true;
    assert!(plan_merge(&rows, "duplicate", "kept").is_err());
    assert!(plan_rename(&rows, "duplicate", "new").is_err());
    let rows = to_rows(&read_back(&backend));
    assert!(plan_merge(&rows, "duplicate", "kept").is_err());
}

#[test]
fn discovery_and_cwd_matching_reuse_https_projects_for_ssh_checkouts() {
    let mut projects = Registry::new();
    let first = git(
        "/first",
        Some("https://example.com/Owner/Repo.git"),
        WorktreeStatus::Primary,
    );
    let second = git(
        "/second",
        Some("ssh://git@EXAMPLE.com:22/Owner/Repo/"),
        WorktreeStatus::Primary,
    );
    discover(&mut projects, std::slice::from_ref(&first));
    assert!(!discover(&mut projects, std::slice::from_ref(&second)).changed());
    let probed = Probed::from([(here("/second"), second)]);
    assert_eq!(
        project_for_cwd(&probed, &projects, &here("/second"))
            .unwrap()
            .name,
        "first"
    );
    assert_eq!(projects["first"].remote, first.remote);
}

// ----------------------------------------------------------------- rows

use crate::store::{self, Backend};
use yinz_tile::prelude::FetchRequest;

fn backend() -> store::Local {
    let backend = store::Local::shared("project_store");
    backend
        .define_all(&store::shared_defs())
        .expect("declared the shared entities");
    backend
}

fn read_back(backend: &impl Backend) -> Registry {
    from_rows(
        &backend
            .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
            .expect("fetched"),
    )
}

/// Serialize a registry the way the old file did, so `parse_legacy` can be
/// tested against input in the shape it will actually meet. Only the tests
/// write this format now.
fn json(projects: &Registry) -> Option<String> {
    serde_json::to_string(projects).ok()
}

/// A folder on this machine.
fn here(path: &str) -> Location {
    Location::new(None::<String>, path)
}

/// A project the user named on a folder, with no Git identity of its own.
fn validated(name: &str, root: &str) -> Project {
    validate(name, root).unwrap()
}

fn checkout(name: &str, root: &str) -> Project {
    Project {
        remote: Some("git@example.com:me/repo".to_owned()),
        worktree_status: Some(WorktreeStatus::Linked),
        common_dir: Some("/work/repo/.git".to_owned()),
        discovered: true,
        ..validate(name, root).unwrap()
    }
}

/// A checkout for `checkout_label`, in the shape `poll::repository` reports.
fn repository(root: &str, common_dir: &str, branch: Option<&str>) -> Repository {
    Repository {
        root: root.to_owned(),
        worktree_status: Some(match root == common_dir.trim_end_matches("/.git") {
            true => WorktreeStatus::Primary,
            false => WorktreeStatus::Linked,
        }),
        common_dir: Some(common_dir.to_owned()),
        branch: branch.map(ToOwned::to_owned),
        ..Repository::default()
    }
}

#[test]
fn a_primary_checkout_is_named_by_its_own_folder_and_branch() {
    let repository = repository("/work/agent", "/work/agent/.git", Some("main"));
    assert_eq!(repository.checkout_label().as_deref(), Some("agent:main"));
}

/// The case the label exists for. The worktree's own folder is `topic`, which
/// says nothing about which repository it belongs to — and this repository
/// keeps its worktrees *inside* the checkout, so the folder above is not the
/// repository either.
#[test]
fn a_linked_worktree_is_named_by_the_repository_it_came_from() {
    let repository = repository(
        "/work/agent/.worktrees/topic",
        "/work/agent/.git",
        Some("topic"),
    );
    assert_eq!(repository.checkout_label().as_deref(), Some("agent:topic"));
}

/// The case the colon separator exists for. A slash is legal in a branch name
/// and the label keeps it whole, so with a slash between the halves
/// `agent/feature/nested` could be read three ways. A colon cannot appear in a
/// ref name, so it marks the one real boundary: `feature/nested` is one branch,
/// not a repository called `feature`.
#[test]
fn a_slashed_branch_survives_intact() {
    let repository = repository("/work/agent", "/work/agent/.git", Some("feature/nested"));
    assert_eq!(
        repository.checkout_label().as_deref(),
        Some("agent:feature/nested")
    );
}

/// A detached HEAD is on no branch. The repository alone is the honest answer;
/// printing the folder in the branch's place would claim something false.
#[test]
fn a_detached_head_is_named_by_its_repository_alone() {
    let repository = repository("/work/agent/.worktrees/topic", "/work/agent/.git", None);
    assert_eq!(repository.checkout_label().as_deref(), Some("agent"));
}

#[test]
fn a_folder_git_did_not_claim_has_no_label() {
    assert_eq!(Repository::folder("/home/me/notes").checkout_label(), None);
}

/// A bare repository *is* its own common dir, conventionally `name.git`. The
/// `.git` is a packaging convention rather than part of the name, so it comes
/// off — the reader is being told which repository, not which folder.
#[test]
fn a_bare_common_dir_names_the_repository_without_its_suffix() {
    let repository = Repository {
        root: "/work/topic".to_owned(),
        worktree_status: Some(WorktreeStatus::Linked),
        common_dir: Some("/srv/git/agent.git".to_owned()),
        branch: Some("topic".to_owned()),
        ..Repository::default()
    };
    assert_eq!(repository.checkout_label().as_deref(), Some("agent:topic"));
}

/// A `common_dir` with no name left in it after either reading. The checkout's
/// own folder is then the only thing there is to say.
#[test]
fn an_unreadable_common_dir_falls_back_to_the_checkout_folder() {
    let repository = Repository {
        root: "/work/topic".to_owned(),
        worktree_status: Some(WorktreeStatus::Linked),
        common_dir: Some("/.git".to_owned()),
        branch: Some("topic".to_owned()),
        ..Repository::default()
    };
    assert_eq!(repository.checkout_label().as_deref(), Some("topic:topic"));
}

#[test]
fn a_project_survives_a_round_trip_through_the_store() {
    let backend = backend();
    let project = checkout("agent", "/work/agent");
    backend.apply(vec![to_mutation(&project)]).expect("written");
    assert_eq!(
        read_back(&backend),
        Registry::from([("agent".to_owned(), project)])
    );
}

#[test]
fn a_project_with_no_git_identity_survives_a_round_trip() {
    let backend = backend();
    let project = validate("notes", "/home/me/notes").unwrap();
    backend.apply(vec![to_mutation(&project)]).expect("written");
    assert_eq!(read_back(&backend)["notes"], project);
}

#[test]
fn both_worktree_statuses_survive_the_text_column() {
    for status in [WorktreeStatus::Primary, WorktreeStatus::Linked] {
        assert_eq!(status.as_str().parse(), Ok(status));
    }
    assert_eq!("something else".parse::<WorktreeStatus>(), Err(()));
}

/// An upsert writes only the attributes it names, so a put that skipped the
/// absent ones would leave a stale remote behind forever.
#[test]
fn a_project_that_loses_its_remote_does_not_keep_the_old_one() {
    let backend = backend();
    backend
        .apply(vec![to_mutation(&checkout("agent", "/work/agent"))])
        .expect("written");

    let plain = validate("agent", "/work/agent").unwrap();
    backend.apply(vec![to_mutation(&plain)]).expect("written");

    let stored = &read_back(&backend)["agent"];
    assert_eq!(stored.remote, None);
    assert_eq!(stored.worktree_status, None);
    assert_eq!(stored.common_dir, None);
    assert!(!stored.discovered);
}

#[test]
fn a_diff_that_changes_nothing_writes_nothing() {
    let registry = Registry::from([("agent".to_owned(), checkout("agent", "/work/agent"))]);
    assert!(diff_mutations(&registry, &registry).is_empty());
}

#[test]
fn a_diff_writes_only_the_projects_that_changed() {
    let before = Registry::from([
        (
            "agent".to_owned(),
            validate("agent", "/work/agent").unwrap(),
        ),
        (
            "notes".to_owned(),
            validate("notes", "/work/notes").unwrap(),
        ),
    ]);
    let mut after = before.clone();
    after.get_mut("notes").unwrap().root = "/work/notes2".to_owned();

    let mutations = diff_mutations(&before, &after);
    assert_eq!(mutations, vec![to_mutation(&after["notes"])]);
}

#[test]
fn a_diff_deletes_the_projects_that_went_away() {
    let before = Registry::from([
        (
            "agent".to_owned(),
            validate("agent", "/work/agent").unwrap(),
        ),
        (
            "notes".to_owned(),
            validate("notes", "/work/notes").unwrap(),
        ),
    ]);
    let after = Registry::from([("agent".to_owned(), before["agent"].clone())]);

    assert_eq!(
        diff_mutations(&before, &after),
        vec![Mutation::delete(
            "project",
            Pred::is_in("name", vec!["notes".to_owned()])
        )]
    );
}

/// The shape every caller will use: materialize, run the existing registry
/// logic against it, diff, apply.
#[test]
fn a_diff_applied_to_the_store_reaches_the_registry_it_described() {
    let backend = backend();
    let before = read_back(&backend);

    let mut after = Registry::new();
    for (name, root) in [("agent", "/work/agent"), ("notes", "/work/notes")] {
        after.insert(name.to_owned(), validate(name, root).unwrap());
    }
    backend
        .apply(diff_mutations(&before, &after))
        .expect("applied");
    assert_eq!(read_back(&backend), after);

    // And the removal half, against rows that are really there.
    let before = read_back(&backend);
    let after = Registry::from([("agent".to_owned(), before["agent"].clone())]);
    backend
        .apply(diff_mutations(&before, &after))
        .expect("applied");
    assert_eq!(read_back(&backend), after);
}

/// Renaming is a delete and a put, and both have to land in one batch or a
/// reader between them sees the project twice or not at all.
#[test]
fn renaming_a_project_leaves_exactly_one_row() {
    let backend = backend();
    let old = validate("agent", "/work/agent").unwrap();
    backend.apply(vec![to_mutation(&old)]).expect("written");

    let new = validate("agent-again", "/work/agent").unwrap();
    backend
        .apply(vec![delete_mutation("agent"), to_mutation(&new)])
        .expect("renamed");

    let stored = read_back(&backend);
    assert_eq!(stored.len(), 1);
    assert!(stored.contains_key("agent-again"));
}

/// Discovery is unchanged: it still takes a `&mut Registry`, and the rows
/// are reached by materializing before and diffing after.
#[test]
fn discovery_runs_against_a_materialized_registry() {
    let backend = backend();
    let repositories = [Repository {
        root: "/work/agent".to_owned(),
        remote: Some("git@example.com:me/agent".to_owned()),
        worktree_status: Some(WorktreeStatus::Primary),
        common_dir: Some("/work/agent/.git".to_owned()),
        ..Repository::default()
    }];

    let mut registry = read_back(&backend);
    let found = discover(&mut registry, &repositories);
    assert!(found.changed(), "a checkout should have made a project");
    backend
        .apply(diff_mutations(&read_back(&backend), &registry))
        .expect("applied");

    let stored = read_back(&backend);
    assert_eq!(stored.len(), 1);
    assert!(stored.values().next().unwrap().discovered);
}

// --------------------------------------------------------------- legacy

#[test]
fn validation_requires_a_name_and_a_clean_absolute_root() {
    assert!(validate("agent", "/work/agent").is_ok());
    assert!(validate("", "/work/agent").is_err());
    assert!(validate("agent", "work/agent").is_err());
    assert!(validate("agent", "/work/../agent").is_err());
}

/// The probe reports a root without a last separator. A user who typed one
/// must still name the same folder, or the two are separate registry keys
/// and `discover` makes a second project for a checkout that already has one.
#[test]
fn validation_takes_a_last_separator_off_a_root() {
    assert_eq!(
        validate("agent", "/work/agent/").unwrap().root,
        "/work/agent"
    );
    assert_eq!(
        validate("agent", "/work//agent").unwrap().root,
        "/work/agent"
    );
}

#[test]
fn folder_matching_stops_at_component_boundaries() {
    let project = validate("agent", "/work/agent").unwrap();
    assert!(contains(&project, &Location::local("/work/agent")));
    assert!(contains(&project, &Location::local("/work/agent/src")));
    assert!(!contains(&project, &Location::local("/work/agent-old")));
}

#[test]
fn the_longest_root_owns_a_folder() {
    let projects = Registry::from([
        ("work".to_owned(), validate("work", "/work").unwrap()),
        (
            "agent".to_owned(),
            validate("agent", "/work/agent").unwrap(),
        ),
    ]);
    assert_eq!(
        for_cwd(&projects, &Location::local("/work/agent/src"))
            .unwrap()
            .name,
        "agent"
    );
}

#[test]
fn invalid_saved_projects_are_dropped() {
    let projects = parse_legacy(
        br#"{"bad":{"name":"bad","root":"relative"},"good":{"name":"good","root":"/work/good"}}"#,
    );
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["good"]);
}

fn folders(cwds: &[&str]) -> Vec<Repository> {
    cwds.iter().map(|cwd| Repository::folder(*cwd)).collect()
}

/// The same roots, as checkouts Git claimed. Each keeps its own common
/// directory, so unrelated roots are not joined by identity.
fn checkouts(roots: &[&str]) -> Vec<Repository> {
    roots
        .iter()
        .map(|root| Repository {
            root: (*root).to_owned(),
            remote: None,
            worktree_status: Some(WorktreeStatus::Primary),
            common_dir: Some(format!("{root}/.git")),
            ..Repository::default()
        })
        .collect()
}

fn registry(projects: &[Project]) -> Registry {
    projects
        .iter()
        .map(|project| (project.name.clone(), project.clone()))
        .collect()
}

#[test]
fn discovery_makes_a_project_from_an_unclaimed_checkout() {
    let mut projects = Registry::new();
    let discovery = discover(&mut projects, &checkouts(&["/work/agent/src"]));

    assert_eq!(discovery.added, vec!["src".to_owned()]);
    assert!(discovery.replaced.is_empty());
    assert_eq!(projects["src"].root, "/work/agent/src");
    assert!(projects["src"].discovered);
}

#[test]
fn a_folder_outside_a_repository_makes_no_project() {
    let mut projects = Registry::new();
    let discovery = discover(&mut projects, &folders(&["/work/notes"]));

    assert!(!discovery.changed());
    assert!(projects.is_empty());
}

#[test]
fn an_agent_inside_a_known_project_starts_no_new_project() {
    let mut projects = registry(&[validate("agent", "/work/agent").unwrap()]);
    let discovery = discover(&mut projects, &checkouts(&["/work/agent/src"]));

    assert!(!discovery.changed());
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["agent"]);
}

#[test]
fn discovery_keeps_a_name_stable_across_polls() {
    let mut projects = Registry::new();
    discover(&mut projects, &checkouts(&["/work/agent"]));
    let again = discover(&mut projects, &checkouts(&["/work/agent"]));

    assert!(!again.changed());
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["agent"]);
}

#[test]
fn discovery_dedupes_a_name_that_is_taken() {
    let mut projects = Registry::new();
    let discovery = discover(&mut projects, &checkouts(&["/a/agent", "/b/agent"]));

    assert_eq!(
        discovery.added,
        vec!["agent".to_owned(), "agent-2".to_owned()]
    );
    assert_eq!(projects["agent"].root, "/a/agent");
    assert_eq!(projects["agent-2"].root, "/b/agent");
}

#[test]
fn a_shallower_root_absorbs_its_discovered_children() {
    let mut projects = Registry::new();
    discover(&mut projects, &checkouts(&["/work/agent/src"]));
    let discovery = discover(&mut projects, &checkouts(&["/work/agent"]));

    assert_eq!(discovery.added, vec!["agent".to_owned()]);
    assert_eq!(
        discovery.replaced,
        vec![("src".to_owned(), "agent".to_owned())]
    );
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["agent"]);
}

#[test]
fn one_pass_puts_the_shallowest_folder_first() {
    let mut projects = Registry::new();
    let discovery = discover(
        &mut projects,
        &checkouts(&["/work/agent/src", "/work/agent"]),
    );

    assert_eq!(discovery.added, vec!["agent".to_owned()]);
    assert!(discovery.replaced.is_empty());
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["agent"]);
}

#[test]
fn discovery_never_absorbs_a_user_project() {
    let mut projects = registry(&[validate("agent", "/work/agent").unwrap()]);
    let discovery = discover(&mut projects, &checkouts(&["/work"]));

    assert_eq!(discovery.added, vec!["work".to_owned()]);
    assert!(discovery.replaced.is_empty());
    assert_eq!(
        for_cwd(&projects, &Location::local("/work/agent/src"))
            .unwrap()
            .name,
        "agent"
    );
    assert_eq!(
        for_cwd(&projects, &Location::local("/work/other"))
            .unwrap()
            .name,
        "work"
    );
}

#[test]
fn a_user_project_replaces_a_discovered_one_on_the_same_root() {
    let mut projects = Registry::new();
    discover(
        &mut projects,
        &[git(
            "/work/agent",
            Some("git@example.com:agent.git"),
            WorktreeStatus::Primary,
        )],
    );
    projects.get_mut("agent").unwrap().worktree_dir = Some("../saved trees".into());
    discover(
        &mut projects,
        &[git(
            "/work/agent",
            Some("git@example.com:agent.git"),
            WorktreeStatus::Primary,
        )],
    );
    assert_eq!(
        projects["agent"].worktree_dir.as_deref(),
        Some("../saved trees")
    );
    let stale = adopt(&mut projects, validate("pocus", "/work/agent").unwrap()).replaced;

    assert_eq!(
        projects["pocus"].worktree_dir.as_deref(),
        Some("../saved trees")
    );
    assert_eq!(stale, Some("agent".to_owned()));
    assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["pocus"]);
    assert!(!projects["pocus"].discovered);
    assert_eq!(
        projects["pocus"].remote.as_deref(),
        Some("git@example.com:agent.git")
    );
}

#[test]
fn discovery_skips_the_filesystem_root() {
    let mut projects = Registry::new();
    let discovery = discover(&mut projects, &checkouts(&["/", ""]));

    assert!(!discovery.changed());
    assert!(projects.is_empty());
}

#[test]
fn saved_projects_without_the_flag_load_as_user_projects() {
    let projects = parse_legacy(br#"{"agent":{"name":"agent","root":"/work/agent"}}"#);
    assert!(!projects["agent"].discovered);
}

/// Discovery used to make a project out of any agent folder. Those saves
/// go away on load; the ones the user named stay, repository or not.
#[test]
fn a_saved_folder_project_from_discovery_is_dropped_on_load() {
    let projects = parse_legacy(
            br#"{
                "notes":{"name":"notes","root":"/work/notes","discovered":true},
                "agent":{"name":"agent","root":"/work/agent","discovered":true,"common_dir":"/work/agent/.git"},
                "papers":{"name":"papers","root":"/work/papers"}
            }"#,
        );

    assert_eq!(
        projects.keys().collect::<Vec<_>>(),
        vec!["agent", "papers"],
        "only the discovered folder project went away"
    );
}

#[test]
fn the_discovered_flag_survives_a_save_and_a_load() {
    let mut projects = Registry::new();
    discover(&mut projects, &checkouts(&["/work/agent"]));
    let saved = parse_legacy(json(&projects).unwrap().as_bytes());

    assert_eq!(saved, projects);
    assert!(saved["agent"].discovered);
}

#[test]
fn forgetting_says_when_a_poll_makes_the_project_again() {
    let mut projects = Registry::new();
    discover(&mut projects, &checkouts(&["/work/agent"]));
    let found = forgotten(&projects["agent"]);

    assert!(found.starts_with("forgot agent"), "got {found}");
    assert!(found.contains("/work/agent"), "got {found}");
    assert_eq!(
        forgotten(&validate("pocus", "/work/pocus").unwrap()),
        "forgot pocus",
        "a project the user named is not made again"
    );
}

// -- checkouts on another machine ------------------------------------

/// The headline case, and the reason any of this exists: a build box has the
/// same layout as the laptop, so a remote agent's folder spells a path a
/// local project owns. Matching it by that path files the agent under work
/// it is not doing.
#[test]
fn a_folder_on_another_machine_is_not_inside_a_local_project() {
    let project = validate("agent", "/work/agent").unwrap();
    let projects = registry(std::slice::from_ref(&project));
    let there = Location::new(Some("build-box"), "/work/agent/src");

    assert!(!contains(&project, &there));
    assert_eq!(for_cwd(&projects, &there), None);
}

/// What a remote checkout *may* match on: the one identity that means the
/// same thing on both machines.
#[test]
fn a_remote_checkout_joins_the_project_its_remote_url_names() {
    let mut project = validate("agent", "/work/agent").unwrap();
    project.remote = Some("git@example.com:me/agent".to_owned());
    let projects = registry(&[project]);

    let there = Repository {
        root: "/srv/agent".to_owned(),
        remote: Some("git@example.com:me/agent".to_owned()),
        worktree_status: Some(WorktreeStatus::Primary),
        common_dir: Some("/srv/agent/.git".to_owned()),
        ..Repository::default()
    }
    .on_host(Some("build-box".to_owned()));

    assert_eq!(
        for_repository(&projects, &there).map(|project| project.name.as_str()),
        Some("agent")
    );
}

/// A `common_dir` is a bare path with nothing in it to say whose filesystem
/// it is. Two machines that both keep their checkout at the same place would
/// otherwise look like one repository.
#[test]
fn a_remote_checkout_with_no_remote_url_matches_nothing() {
    let mut project = validate("agent", "/work/agent").unwrap();
    project.common_dir = Some("/work/agent/.git".to_owned());
    let projects = registry(&[project]);

    let there = Repository {
        root: "/work/agent".to_owned(),
        worktree_status: Some(WorktreeStatus::Primary),
        common_dir: Some("/work/agent/.git".to_owned()),
        ..Repository::default()
    }
    .on_host(Some("build-box".to_owned()));

    assert_eq!(for_repository(&projects, &there), None);
    // ...while the identical checkout on this machine still does.
    let here = Repository {
        host: None,
        ..there
    };
    assert!(for_repository(&projects, &here).is_some());
}

/// Discovery writes a `root` into a stored row, and a path from another
/// machine names a folder here that is missing or somebody else's.
#[test]
fn discovery_ignores_a_checkout_on_another_machine() {
    let mut projects = Registry::new();
    let there =
        git("/work/agent", None, WorktreeStatus::Primary).on_host(Some("build-box".to_owned()));

    let discovery = discover(&mut projects, &[there]);

    assert!(discovery.added.is_empty(), "made a project: {discovery:?}");
    assert!(projects.is_empty(), "{projects:?}");
}

fn git(root: &str, remote: Option<&str>, status: WorktreeStatus) -> Repository {
    Repository {
        root: root.to_owned(),
        remote: remote.map(str::to_owned),
        worktree_status: Some(status),
        common_dir: Some("/repos/agent/.git".to_owned()),
        ..Repository::default()
    }
}

/// Git puts a worktree wherever it is asked to, and inside the checkout it
/// came from is an ordinary place to ask for. Discovery makes no second
/// project out of one — the containing root already owns the folder — so the
/// two agents share a project header and only their branches tell them
/// apart. That is what makes the branch the thing worth getting right.
#[test]
fn a_worktree_nested_in_its_primary_checkout_is_one_project() {
    let mut projects = Registry::new();
    let primary = git("/work/agent", None, WorktreeStatus::Primary);
    let nested = git(
        "/work/agent/.claude/worktrees/topic",
        None,
        WorktreeStatus::Linked,
    );

    discover(&mut projects, &[primary.clone(), nested.clone()]);

    assert_eq!(projects.len(), 1, "one project: {projects:?}");
    let project = projects.values().next().expect("a project");
    assert_eq!(project.root, "/work/agent");
    assert_eq!(
        for_cwd(&projects, &Location::local(&nested.root)).map(|p| &p.name),
        Some(&project.name)
    );
}

#[test]
fn worktrees_with_the_same_remote_are_one_project() {
    let mut projects = Registry::new();
    discover(
        &mut projects,
        &[git(
            "/worktrees/feature",
            Some("git@example.com:agent.git"),
            WorktreeStatus::Linked,
        )],
    );
    let discovery = discover(
        &mut projects,
        &[git(
            "/work/agent",
            Some("git@example.com:agent.git"),
            WorktreeStatus::Primary,
        )],
    );

    assert_eq!(projects.len(), 1);
    assert_eq!(projects["feature"].root, "/work/agent");
    assert_eq!(
        projects["feature"].worktree_status,
        Some(WorktreeStatus::Primary)
    );
    assert_eq!(
        discovery.root_moves,
        vec![("/worktrees/feature".to_owned(), "/work/agent".to_owned())]
    );
}

#[test]
fn remote_less_worktrees_share_the_common_git_directory() {
    let mut projects = Registry::new();
    discover(
        &mut projects,
        &[
            git("/a/agent", None, WorktreeStatus::Primary),
            git("/b/feature", None, WorktreeStatus::Linked),
        ],
    );

    assert_eq!(projects.len(), 1);
    assert_eq!(
        for_repository(&projects, &git("/b/feature", None, WorktreeStatus::Linked))
            .unwrap()
            .name,
        "agent"
    );
}

/// A tab is named by the repository alone. Two worktrees of one repository
/// are one project's worth of work, and the branch is exactly what would
/// split them into two names.
#[test]
fn a_tab_is_named_by_the_repository_without_the_branch() {
    let primary = repository("/work/agent", "/work/agent/.git", Some("main"));
    assert_eq!(primary.checkout_name().as_deref(), Some("agent"));

    let worktree = repository("/work/trees/topic", "/work/agent/.git", Some("topic"));
    assert_eq!(
        worktree.checkout_name().as_deref(),
        Some("agent"),
        "the primary's name, not the worktree folder's",
    );

    assert_eq!(Repository::folder("/home/me/notes").checkout_name(), None);
}

/// The registry is asked first: the user renamed the folder for a reason,
/// and that name is the one they already use for this work.
#[test]
fn a_registered_project_labels_a_folder_over_its_repository() {
    let probed = Probed::from([(
        here("/work/agent"),
        repository("/work/agent", "/work/agent/.git", Some("main")),
    )]);
    let mut projects = Registry::new();
    projects.insert(
        "the-workspace".to_owned(),
        validated("the-workspace", "/work/agent"),
    );

    assert_eq!(
        label_for_cwd(&probed, &projects, &here("/work/agent")).as_deref(),
        Some("the-workspace"),
    );
}

/// A checkout nobody registered still names a tab. That is the whole reason
/// the label is a wider question than the project.
#[test]
fn an_unregistered_checkout_labels_itself() {
    let probed = Probed::from([(
        here("/somewhere/yinz"),
        repository("/somewhere/yinz", "/somewhere/yinz/.git", Some("main")),
    )]);

    assert_eq!(
        label_for_cwd(&probed, &Registry::new(), &here("/somewhere/yinz")).as_deref(),
        Some("yinz"),
    );
}

/// A registered folder needs no Git confirmation to name a tab.
#[test]
fn a_plain_folder_uses_its_explicit_project_label() {
    let probed = Probed::from([(
        here("/work/agent/notes"),
        Repository::folder("/work/agent/notes"),
    )]);
    let mut projects = Registry::new();
    projects.insert("agent".to_owned(), validated("agent", "/work/agent"));

    assert_eq!(
        label_for_cwd(&probed, &projects, &here("/work/agent/notes")),
        Some("agent".to_owned()),
    );
}

#[test]
fn the_most_specific_explicit_root_names_a_folder_before_git_answers() {
    let mut projects = Registry::from([
        ("workspace".into(), validated("workspace", "/work")),
        ("notes".into(), validated("notes", "/work/notes")),
        ("found".into(), validated("found", "/work/notes/deep")),
    ]);
    projects.get_mut("found").unwrap().discovered = true;
    let cwd = here("/work/notes/deep/src");
    assert_eq!(
        label_for_cwd(&Probed::new(), &projects, &cwd).as_deref(),
        Some("notes")
    );

    // Even a different Git identity loses to the explicit local folder.
    let mut other = validated("other", "/elsewhere");
    other.common_dir = Some("/elsewhere/.git".into());
    projects.insert("other".into(), other);
    let probed = Probed::from([(
        cwd.clone(),
        repository(&cwd.path, "/elsewhere/.git", Some("main")),
    )]);
    assert_eq!(
        label_for_cwd(&probed, &projects, &cwd).as_deref(),
        Some("notes")
    );
    assert_eq!(
        label_for_cwd(&Probed::new(), &projects, &here("/work/notes-old")).as_deref(),
        Some("workspace")
    );
    assert_eq!(
        label_for_cwd(
            &Probed::new(),
            &projects,
            &Location::new(Some("remote"), &cwd.path)
        ),
        None
    );
}

/// A subdirectory of a probed checkout borrows the answer from above it,
/// rather than losing its project until a fresh probe lands.
#[test]
fn a_subdirectory_borrows_the_label_of_the_checkout_above_it() {
    let probed = Probed::from([(
        here("/work/agent"),
        repository("/work/agent", "/work/agent/.git", Some("main")),
    )]);

    assert_eq!(
        label_for_cwd(&probed, &Registry::new(), &here("/work/agent/src")).as_deref(),
        Some("agent"),
    );
}

/// A folder on another machine is never filed under the local project that
/// happens to sit at the same path. A build box laid out like the laptop is
/// the ordinary case, not a corner one.
#[test]
fn a_remote_folder_is_not_labelled_by_a_local_project_at_the_same_path() {
    let there = Location::new(Some("build-box".to_owned()), "/work/agent");
    let probed = Probed::from([(
        there.clone(),
        Repository {
            host: Some("build-box".to_owned()),
            ..repository("/work/agent", "/work/agent/.git", Some("main"))
        },
    )]);
    let mut projects = Registry::new();
    projects.insert(
        "the-workspace".to_owned(),
        validated("the-workspace", "/work/agent"),
    );

    assert_eq!(
        label_for_cwd(&probed, &projects, &there).as_deref(),
        Some("agent"),
        "named by its own repository, never by the local project at that path",
    );
}
