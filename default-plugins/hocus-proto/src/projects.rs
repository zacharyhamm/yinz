//! Persistent projects and the repositories or folders that identify them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

const FILE: &str = "/cache/projects.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_status: Option<WorktreeStatus>,
    /// Used only to join worktrees of repositories that have no remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_dir: Option<String>,
    /// True when a poll made this project from an agent folder, not the user.
    ///
    /// A saved project from before this field existed has no key, so it loads
    /// as a user project. That is the safe direction: discovery never removes
    /// a user project.
    #[serde(default)]
    pub discovered: bool,
}

impl Project {
    /// True when this project carries the Git identity of a checkout.
    ///
    /// A user project need not have one — the user can name any folder. A
    /// discovered project always does, because discovery only runs on
    /// checkouts.
    pub fn is_checkout(&self) -> bool {
        self.worktree_status.is_some() || self.common_dir.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Primary,
    Linked,
}

/// What Git says about one checkout. Missing Git fields mean an ordinary folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_status: Option<WorktreeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_dir: Option<String>,
    /// The branch this checkout has out. Unlike the fields above it goes stale —
    /// a checkout in the folder changes it — so whoever shows it is responsible
    /// for asking for a fresh probe.
    ///
    /// `None` on a detached HEAD, which names no branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl Repository {
    /// A folder Git did not claim: probed, and not a checkout.
    ///
    /// Still worth recording, because it is what tells the daemon it has
    /// already run `git rev-parse` here and need not run it again.
    pub fn folder(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            remote: None,
            worktree_status: None,
            common_dir: None,
            branch: None,
        }
    }

    /// True when Git claimed this folder. Only a checkout makes a project.
    pub fn is_checkout(&self) -> bool {
        self.worktree_status.is_some() || self.common_dir.is_some()
    }
}

pub type Registry = BTreeMap<String, Project>;

pub fn load() -> Registry {
    parse(&fs::read(FILE).unwrap_or_default())
}

fn parse(json: &[u8]) -> Registry {
    let projects: Registry = serde_json::from_slice(json).unwrap_or_default();
    projects
        .into_values()
        .filter_map(|saved| {
            let mut project = validate(&saved.name, &saved.root).ok()?;
            project.discovered = saved.discovered;
            project.remote = saved.remote;
            project.worktree_status = saved.worktree_status;
            project.common_dir = saved.common_dir;
            // Older polls made a project out of any folder an agent sat in.
            // Discovery is for checkouts now, so those go away here rather
            // than living on as projects nothing would ever make again. A
            // project the user named stays, whatever its folder is.
            (!project.discovered || project.is_checkout()).then_some(project)
        })
        .map(|project| (project.name.clone(), project))
        .collect()
}

pub fn json(projects: &Registry) -> Option<String> {
    serde_json::to_string(projects).ok()
}

pub fn write(json: &[u8]) {
    let _ = fs::write(FILE, json);
}

pub fn validate(name: &str, root: &str) -> Result<Project, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("project name must contain visible characters".to_owned());
    }
    let root = Path::new(root);
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err("project root must be an absolute path without . or ..".to_owned());
    }
    Ok(Project {
        name: name.to_owned(),
        root: root.to_string_lossy().into_owned(),
        remote: None,
        worktree_status: None,
        common_dir: None,
        discovered: false,
    })
}

pub fn contains(project: &Project, cwd: &str) -> bool {
    Path::new(cwd).starts_with(&project.root)
}

pub fn for_cwd<'a>(projects: &'a Registry, cwd: &str) -> Option<&'a Project> {
    projects
        .values()
        .filter(|project| contains(project, cwd))
        .max_by_key(|project| Path::new(&project.root).components().count())
}

fn same_repository(project: &Project, repository: &Repository) -> bool {
    match (&project.remote, &repository.remote) {
        (Some(left), Some(right)) => left == right,
        (None, None) => project.common_dir.is_some() && project.common_dir == repository.common_dir,
        _ => false,
    }
}

/// The project for a checkout, unless explicit same-remote projects make it ambiguous.
pub fn for_repository<'a>(projects: &'a Registry, repository: &Repository) -> Option<&'a Project> {
    let mut matches = projects
        .values()
        .filter(|project| same_repository(project, repository));
    let project = matches.next()?;
    matches.next().is_none().then_some(project)
}

/// What one discovery pass changed.
///
/// The caller holds project names in other places — the tab assignments and the
/// active project — so a pass must report what it removed, not only what it
/// added.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    /// The names of the projects the pass made.
    pub added: Vec<String>,
    /// One pair for each discovered project a shallower root replaced: the name
    /// that went away, then the name that took its folders.
    pub replaced: Vec<(String, String)>,
    /// Canonical roots moved from a linked checkout to its primary checkout.
    pub root_moves: Vec<(String, String)>,
    /// Existing projects whose repository metadata was filled in.
    pub updated: Vec<String>,
}

impl Discovery {
    pub fn changed(&self) -> bool {
        !self.added.is_empty()
            || !self.replaced.is_empty()
            || !self.root_moves.is_empty()
            || !self.updated.is_empty()
    }
}

/// The last part of a folder, as a project name.
///
/// `None` for the filesystem root, which has no last part. A project there
/// would own every folder and make later discovery impossible.
fn leaf_name(root: &str) -> Option<String> {
    let leaf = Path::new(root).file_name()?.to_string_lossy().into_owned();
    (!leaf.trim().is_empty() && !leaf.chars().any(char::is_control)).then_some(leaf)
}

/// `base`, or `base-2`, `base-3` and so on when `base` is taken.
fn unique_name(projects: &Registry, base: &str) -> String {
    if !projects.contains_key(base) {
        return base.to_owned();
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|name| !projects.contains_key(name))
        .unwrap_or_else(|| base.to_owned())
}

/// Make a project for each checkout that no project owns yet.
///
/// Only a Git checkout makes a project. A folder outside a repository is
/// somewhere an agent happened to be started, not work worth remembering for
/// good under a name taken from its last path component.
///
/// The checkouts are sorted with the shortest root first, so the result does
/// not change with the order the panes came in. A folder that an existing
/// project owns makes nothing.
pub fn discover(projects: &mut Registry, repositories: &[Repository]) -> Discovery {
    let mut roots: Vec<&Repository> = repositories
        .iter()
        .filter(|repository| repository.is_checkout())
        .filter(|repository| !repository.root.trim().is_empty())
        .collect();
    roots.sort_by_key(|repository| {
        (
            Path::new(&repository.root).components().count(),
            repository.root.as_str(),
        )
    });
    roots.dedup_by_key(|repository| repository.root.as_str());

    let mut discovery = Discovery::default();
    for repository in roots {
        if let Some(name) = for_cwd(projects, &repository.root)
            .and_then(|project| (project.root == repository.root).then(|| project.name.clone()))
        {
            let Some(project) = projects.get_mut(&name) else {
                continue;
            };
            if project.remote.is_none() && project.common_dir.is_none() {
                project.remote.clone_from(&repository.remote);
                project.worktree_status = repository.worktree_status;
                project.common_dir.clone_from(&repository.common_dir);
                if repository.worktree_status.is_some() {
                    discovery.updated.push(name);
                }
            }
            continue;
        }
        if for_cwd(projects, &repository.root).is_some() {
            continue;
        }
        let identity_matches: Vec<String> = projects
            .values()
            .filter(|project| same_repository(project, repository))
            .map(|project| project.name.clone())
            .collect();
        if identity_matches.len() > 1 {
            continue;
        }
        if let Some(name) = identity_matches.first() {
            let Some(project) = projects.get_mut(name) else {
                continue;
            };
            if project.discovered
                && project.worktree_status == Some(WorktreeStatus::Linked)
                && repository.worktree_status == Some(WorktreeStatus::Primary)
            {
                let old = std::mem::replace(&mut project.root, repository.root.clone());
                project.remote.clone_from(&repository.remote);
                project.worktree_status = repository.worktree_status;
                project.common_dir.clone_from(&repository.common_dir);
                discovery.root_moves.push((old, project.root.clone()));
            }
            continue;
        }
        let Some(leaf) = leaf_name(&repository.root) else {
            continue;
        };
        let Ok(mut project) = validate(&leaf, &repository.root) else {
            continue;
        };
        project.remote.clone_from(&repository.remote);
        project.worktree_status = repository.worktree_status;
        project.common_dir.clone_from(&repository.common_dir);
        let name = unique_name(projects, &project.name);

        // A shallower folder takes the folders of the projects that polls found
        // below it. The user projects below it stay, because `for_cwd` gives a
        // folder to the longest root.
        let absorbed: Vec<String> = projects
            .values()
            .filter(|below| {
                below.discovered
                    && below.root != project.root
                    && Path::new(&below.root).starts_with(&project.root)
            })
            .map(|below| below.name.clone())
            .collect();
        for below in absorbed {
            projects.remove(&below);
            discovery.replaced.push((below, name.clone()));
        }

        projects.insert(
            name.clone(),
            Project {
                name: name.clone(),
                discovered: true,
                ..project
            },
        );
        discovery.added.push(name);
    }
    discovery
}

/// Add a project the user named.
///
/// A discovered project on the same folder goes away, because the user just
/// gave that folder a better name. The name that went away comes back, so the
/// caller can move the tabs that point at it.
pub fn adopt(projects: &mut Registry, mut project: Project) -> Option<String> {
    let stale = projects
        .values()
        .find(|other| other.discovered && other.root == project.root && other.name != project.name)
        .map(|other| {
            project.remote.clone_from(&other.remote);
            project.worktree_status = other.worktree_status;
            project.common_dir.clone_from(&other.common_dir);
            other.name.clone()
        });
    if let Some(name) = &stale {
        projects.remove(name);
    }
    projects.insert(project.name.clone(), project);
    stale
}

/// What to say for a project that was just forgotten.
///
/// A discovered project is made again by the next poll that sees an agent in
/// its checkout. Saying so is what keeps the caller from forgetting it a second
/// time and expecting a different outcome.
pub fn forgotten(project: &Project) -> String {
    let name = &project.name;
    if project.discovered {
        format!(
            "forgot {name}; a poll makes it again while an agent runs in {}",
            project.root
        )
    } else {
        format!("forgot {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_requires_a_name_and_a_clean_absolute_root() {
        assert!(validate("hocus", "/work/hocus").is_ok());
        assert!(validate("", "/work/hocus").is_err());
        assert!(validate("hocus", "work/hocus").is_err());
        assert!(validate("hocus", "/work/../hocus").is_err());
    }

    #[test]
    fn folder_matching_stops_at_component_boundaries() {
        let project = validate("hocus", "/work/hocus").unwrap();
        assert!(contains(&project, "/work/hocus"));
        assert!(contains(&project, "/work/hocus/src"));
        assert!(!contains(&project, "/work/hocus-old"));
    }

    #[test]
    fn the_longest_root_owns_a_folder() {
        let projects = Registry::from([
            ("work".to_owned(), validate("work", "/work").unwrap()),
            (
                "hocus".to_owned(),
                validate("hocus", "/work/hocus").unwrap(),
            ),
        ]);
        assert_eq!(for_cwd(&projects, "/work/hocus/src").unwrap().name, "hocus");
    }

    #[test]
    fn invalid_saved_projects_are_dropped() {
        let projects = parse(
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
                branch: None,
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
        let discovery = discover(&mut projects, &checkouts(&["/work/hocus/src"]));

        assert_eq!(discovery.added, vec!["src".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(projects["src"].root, "/work/hocus/src");
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
        let mut projects = registry(&[validate("hocus", "/work/hocus").unwrap()]);
        let discovery = discover(&mut projects, &checkouts(&["/work/hocus/src"]));

        assert!(!discovery.changed());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_keeps_a_name_stable_across_polls() {
        let mut projects = Registry::new();
        discover(&mut projects, &checkouts(&["/work/hocus"]));
        let again = discover(&mut projects, &checkouts(&["/work/hocus"]));

        assert!(!again.changed());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_dedupes_a_name_that_is_taken() {
        let mut projects = Registry::new();
        let discovery = discover(&mut projects, &checkouts(&["/a/hocus", "/b/hocus"]));

        assert_eq!(
            discovery.added,
            vec!["hocus".to_owned(), "hocus-2".to_owned()]
        );
        assert_eq!(projects["hocus"].root, "/a/hocus");
        assert_eq!(projects["hocus-2"].root, "/b/hocus");
    }

    #[test]
    fn a_shallower_root_absorbs_its_discovered_children() {
        let mut projects = Registry::new();
        discover(&mut projects, &checkouts(&["/work/hocus/src"]));
        let discovery = discover(&mut projects, &checkouts(&["/work/hocus"]));

        assert_eq!(discovery.added, vec!["hocus".to_owned()]);
        assert_eq!(
            discovery.replaced,
            vec![("src".to_owned(), "hocus".to_owned())]
        );
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn one_pass_puts_the_shallowest_folder_first() {
        let mut projects = Registry::new();
        let discovery = discover(
            &mut projects,
            &checkouts(&["/work/hocus/src", "/work/hocus"]),
        );

        assert_eq!(discovery.added, vec!["hocus".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_never_absorbs_a_user_project() {
        let mut projects = registry(&[validate("hocus", "/work/hocus").unwrap()]);
        let discovery = discover(&mut projects, &checkouts(&["/work"]));

        assert_eq!(discovery.added, vec!["work".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(for_cwd(&projects, "/work/hocus/src").unwrap().name, "hocus");
        assert_eq!(for_cwd(&projects, "/work/other").unwrap().name, "work");
    }

    #[test]
    fn a_user_project_replaces_a_discovered_one_on_the_same_root() {
        let mut projects = Registry::new();
        discover(
            &mut projects,
            &[git(
                "/work/hocus",
                Some("git@example.com:hocus.git"),
                WorktreeStatus::Primary,
            )],
        );
        let stale = adopt(&mut projects, validate("pocus", "/work/hocus").unwrap());

        assert_eq!(stale, Some("hocus".to_owned()));
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["pocus"]);
        assert!(!projects["pocus"].discovered);
        assert_eq!(
            projects["pocus"].remote.as_deref(),
            Some("git@example.com:hocus.git")
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
        let projects = parse(br#"{"hocus":{"name":"hocus","root":"/work/hocus"}}"#);
        assert!(!projects["hocus"].discovered);
    }

    /// Discovery used to make a project out of any agent folder. Those saves
    /// go away on load; the ones the user named stay, repository or not.
    #[test]
    fn a_saved_folder_project_from_discovery_is_dropped_on_load() {
        let projects = parse(
            br#"{
                "notes":{"name":"notes","root":"/work/notes","discovered":true},
                "hocus":{"name":"hocus","root":"/work/hocus","discovered":true,"common_dir":"/work/hocus/.git"},
                "papers":{"name":"papers","root":"/work/papers"}
            }"#,
        );

        assert_eq!(
            projects.keys().collect::<Vec<_>>(),
            vec!["hocus", "papers"],
            "only the discovered folder project went away"
        );
    }

    #[test]
    fn the_discovered_flag_survives_a_save_and_a_load() {
        let mut projects = Registry::new();
        discover(&mut projects, &checkouts(&["/work/hocus"]));
        let saved = parse(json(&projects).unwrap().as_bytes());

        assert_eq!(saved, projects);
        assert!(saved["hocus"].discovered);
    }

    #[test]
    fn forgetting_says_when_a_poll_makes_the_project_again() {
        let mut projects = Registry::new();
        discover(&mut projects, &checkouts(&["/work/hocus"]));
        let found = forgotten(&projects["hocus"]);

        assert!(found.starts_with("forgot hocus"), "got {found}");
        assert!(found.contains("/work/hocus"), "got {found}");
        assert_eq!(
            forgotten(&validate("pocus", "/work/pocus").unwrap()),
            "forgot pocus",
            "a project the user named is not made again"
        );
    }

    fn git(root: &str, remote: Option<&str>, status: WorktreeStatus) -> Repository {
        Repository {
            root: root.to_owned(),
            remote: remote.map(str::to_owned),
            worktree_status: Some(status),
            common_dir: Some("/repos/hocus/.git".to_owned()),
            branch: None,
        }
    }

    #[test]
    fn worktrees_with_the_same_remote_are_one_project() {
        let mut projects = Registry::new();
        discover(
            &mut projects,
            &[git(
                "/worktrees/feature",
                Some("git@example.com:hocus.git"),
                WorktreeStatus::Linked,
            )],
        );
        let discovery = discover(
            &mut projects,
            &[git(
                "/work/hocus",
                Some("git@example.com:hocus.git"),
                WorktreeStatus::Primary,
            )],
        );

        assert_eq!(projects.len(), 1);
        assert_eq!(projects["feature"].root, "/work/hocus");
        assert_eq!(
            projects["feature"].worktree_status,
            Some(WorktreeStatus::Primary)
        );
        assert_eq!(
            discovery.root_moves,
            vec![("/worktrees/feature".to_owned(), "/work/hocus".to_owned())]
        );
    }

    #[test]
    fn remote_less_worktrees_share_the_common_git_directory() {
        let mut projects = Registry::new();
        discover(
            &mut projects,
            &[
                git("/a/hocus", None, WorktreeStatus::Primary),
                git("/b/feature", None, WorktreeStatus::Linked),
            ],
        );

        assert_eq!(projects.len(), 1);
        assert_eq!(
            for_repository(&projects, &git("/b/feature", None, WorktreeStatus::Linked))
                .unwrap()
                .name,
            "hocus"
        );
    }
}
