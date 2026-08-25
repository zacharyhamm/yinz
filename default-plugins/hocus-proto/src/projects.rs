//! Persistent projects and the repositories or folders that identify them.

use crate::location::Location;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use zellij_tile::prelude::{DataRows, DataValue, Mutation, Pred};

/// The file the registry lived in before the store. Read once, by the import.
pub const FILE: &str = "/cache/projects.json";

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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub root: String,
    /// The machine this checkout is on, or `None` for this one.
    ///
    /// Redundant with the key of the map a repository is usually found in, and
    /// kept anyway: [`discover`] and [`for_repository`] are handed bare
    /// repositories with no key in sight, and those two are precisely the
    /// functions that must refuse to match a checkout on another machine by
    /// path. `root` and `common_dir` are paths over *there*, and comparing them
    /// to a project's is comparing two different filesystems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
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
            ..Self::default()
        }
    }

    /// Say which machine this checkout is on. `None` leaves it on this one.
    pub fn on_host(mut self, host: Option<String>) -> Self {
        self.host = host;
        self
    }

    /// True when Git claimed this folder. Only a checkout makes a project.
    pub fn is_checkout(&self) -> bool {
        self.worktree_status.is_some() || self.common_dir.is_some()
    }

    /// `repo/branch` — where this checkout is, in the fewest words that still
    /// tell two worktrees of one repository apart.
    ///
    /// `None` for a folder Git did not claim: there is nothing to say, and a
    /// bare folder name where the reader expects a checkout would read as one.
    ///
    /// The repository half is the *primary* checkout, not this folder, which is
    /// the whole reason it is worth printing: a worktree kept at
    /// `zellij/.worktrees/topic` is named `topic` by its own folder, and that
    /// says nothing about which repository it belongs to. See
    /// [`repository_name`] for how the primary is read out of `common_dir`.
    ///
    /// A detached HEAD is on no branch, so the repository stands alone. Naming
    /// the folder there instead would say "branch" about something that is not
    /// one — the same stance `poll::repository` takes when it refuses to read
    /// Git's `HEAD` as a branch name.
    pub fn checkout_label(&self) -> Option<String> {
        if !self.is_checkout() {
            return None;
        }
        // A checkout at `/` names no folder, and a lone slash on a pane frame is
        // noise rather than an address.
        let repository = self
            .common_dir
            .as_deref()
            .and_then(repository_name)
            .or_else(|| Some(folder_name(&self.root)).filter(|name| !name.is_empty()))?;
        match &self.branch {
            Some(branch) => Some(format!("{repository}/{branch}")),
            None => Some(repository.to_owned()),
        }
    }
}

/// The repository a `common_dir` belongs to, by its folder.
///
/// Two shapes, because Git has two. A worktree's common dir is the primary
/// checkout's `.git`, so the repository is the folder above it. A bare
/// repository *is* its common dir, conventionally `name.git`, so the repository
/// is that folder with the suffix taken off. `None` when neither reading leaves
/// a name — then the caller falls back to the checkout's own folder.
fn repository_name(common_dir: &str) -> Option<&str> {
    let dir = common_dir.trim_end_matches('/');
    let name = match folder_name(dir) {
        ".git" => parent_of(dir).map(folder_name).unwrap_or_default(),
        bare => bare.strip_suffix(".git").unwrap_or(bare),
    };
    (!name.is_empty()).then_some(name)
}

/// The last segment of a path, with any trailing separators ignored.
fn folder_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
}

/// The path above `path`, or `None` when there is nothing above it.
fn parent_of(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let (parent, _) = trimmed.rsplit_once('/')?;
    (!parent.is_empty()).then_some(parent)
}

pub type Registry = BTreeMap<String, Project>;

/// Read the JSON file the registry used to live in, for the one-time import.
///
/// The revalidation stays here rather than moving to the row layer, and this is
/// the reason: it exists because a file could hold anything, and after the
/// import nothing does. Rows are written through [`validate`], so a row is valid
/// by construction and re-checking it on every read would be work that can only
/// ever succeed.
pub fn parse_legacy(json: &[u8]) -> Registry {
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

pub fn validate(name: &str, root: &str) -> Result<Project, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("project name must contain visible characters".to_owned());
    }
    // Normalized, not merely checked: a root that kept a last separator would
    // key the registry differently from the same folder as the Git probe
    // reports it, and `discover` compares the two as strings.
    let root = crate::paths::normalize(root)
        .map_err(|_| "project root must be an absolute path without . or ..".to_owned())?;
    Ok(Project {
        name: name.to_owned(),
        root,
        remote: None,
        worktree_status: None,
        common_dir: None,
        discovered: false,
    })
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------
//
// A project is one row keyed by name, because the registry is keyed by name and
// every caller looks one up that way. Neither the name nor the root is stable —
// `adopt` renames and discovery moves roots — so there is no stabler choice to
// make instead.

/// The columns a registry needs. `name` is the uid as well as a column: a uid
/// cannot be selected or sorted on.
pub const ROW_COLUMNS: &[&str] = &[
    "name",
    "root",
    "remote",
    "worktree_status",
    "common_dir",
    "discovered",
];

impl WorktreeStatus {
    /// The same snake_case spelling serde writes, so the column reads the way
    /// the old file did and can be understood without a mapping table.
    pub fn as_str(&self) -> &'static str {
        match self {
            WorktreeStatus::Primary => "primary",
            WorktreeStatus::Linked => "linked",
        }
    }
}

impl std::str::FromStr for WorktreeStatus {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "primary" => Ok(WorktreeStatus::Primary),
            "linked" => Ok(WorktreeStatus::Linked),
            _ => Err(()),
        }
    }
}

fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<String> {
    match rows.get(row, column) {
        Some(DataValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Build the registry these rows describe.
///
/// A row missing a name or a root is skipped rather than guessed at. That should
/// be impossible — both are `required` — but the alternative is a project with
/// an empty name that nothing can address and nothing can remove.
pub fn from_rows(rows: &DataRows) -> Registry {
    rows.rows
        .iter()
        .filter_map(|row| {
            let (Some(name), Some(root)) = (text_at(rows, row, "name"), text_at(rows, row, "root"))
            else {
                return None;
            };
            Some((
                name.clone(),
                Project {
                    name,
                    root,
                    remote: text_at(rows, row, "remote"),
                    worktree_status: text_at(rows, row, "worktree_status")
                        .as_deref()
                        .and_then(|status| status.parse().ok()),
                    common_dir: text_at(rows, row, "common_dir"),
                    discovered: matches!(rows.get(row, "discovered"), Some(DataValue::Bool(true))),
                },
            ))
        })
        .collect()
}

/// The write that stores one project.
///
/// Every attribute is set on every put, including the absent ones. An upsert
/// only writes the attributes it names, so a project that loses its remote would
/// otherwise keep the old value forever.
pub fn to_mutation(project: &Project) -> Mutation {
    Mutation::put("project")
        .uid(project.name.clone())
        .set("name", project.name.clone())
        .set("root", project.root.clone())
        .set("remote", project.remote.clone())
        .set(
            "worktree_status",
            project.worktree_status.map(|s| s.as_str().to_owned()),
        )
        .set("common_dir", project.common_dir.clone())
        .set("discovered", project.discovered)
        .build()
}

/// The writes that turn `before` into `after`.
///
/// Discovery works on a whole registry and reports what it changed, so the
/// simplest correct thing is to hand it a materialized copy and diff the result.
/// Untouched projects produce no mutation at all, which matters more than it
/// looks: a watch only fires when the rows actually differ, so a discovery pass
/// that changes nothing wakes nobody.
pub fn diff_mutations(before: &Registry, after: &Registry) -> Vec<Mutation> {
    let mut mutations = Vec::new();
    for (name, project) in after {
        if before.get(name) != Some(project) {
            mutations.push(to_mutation(project));
        }
    }
    let gone: Vec<String> = before
        .keys()
        .filter(|name| !after.contains_key(*name))
        .cloned()
        .collect();
    if !gone.is_empty() {
        mutations.push(Mutation::delete("project", Pred::is_in("name", gone)));
    }
    mutations
}

/// A registry as the rows a fetch of it would return.
///
/// The inverse of [`from_rows`], for anything that has a registry in hand and
/// needs to hand it to something row-shaped — which in practice means tests
/// driving the change handler without going near a store.
pub fn to_rows(projects: &Registry) -> DataRows {
    DataRows {
        columns: ROW_COLUMNS.iter().map(|c| (*c).to_owned()).collect(),
        rows: projects
            .values()
            .map(|project| {
                vec![
                    DataValue::Text(project.name.clone()),
                    DataValue::Text(project.root.clone()),
                    project
                        .remote
                        .clone()
                        .map_or(DataValue::Null, DataValue::Text),
                    project
                        .worktree_status
                        .map_or(DataValue::Null, |s| DataValue::Text(s.as_str().to_owned())),
                    project
                        .common_dir
                        .clone()
                        .map_or(DataValue::Null, DataValue::Text),
                    DataValue::Bool(project.discovered),
                ]
            })
            .collect(),
        truncated: false,
        next_cursor: None,
    }
}

/// The write that forgets one project by name.
pub fn delete_mutation(name: &str) -> Mutation {
    Mutation::delete("project", Pred::eq("name", name))
}

/// Whether a folder is inside a project's root.
///
/// A project's `root` is a path on *this* machine — the user named it here, or
/// discovery made it from a checkout here — so only a folder on this machine can
/// be inside it. A folder on another machine that happens to spell the same path
/// is a different place, and matching it would file a remote agent under
/// whichever local project happens to sit at that path. That is the ordinary
/// case, not a corner one: someone's build box has their layout on it.
pub fn contains(project: &Project, cwd: &Location) -> bool {
    cwd.host.is_none() && Path::new(&cwd.path).starts_with(&project.root)
}

pub fn for_cwd<'a>(projects: &'a Registry, cwd: &Location) -> Option<&'a Project> {
    projects
        .values()
        .filter(|project| contains(project, cwd))
        .max_by_key(|project| Path::new(&project.root).components().count())
}

/// Whether a project and a checkout are the same repository.
///
/// A remote URL is the one identity that survives crossing a machine: the same
/// repository checked out on a build box and here is the same work, and saying
/// so is the whole point of letting a remote pane have a project at all.
///
/// The fallback for a repository with no remote is not: a `common_dir` is a bare
/// path, with nothing in it to say which filesystem it is on, so comparing one
/// from over there to one from here is comparing two strings that mean different
/// things. A hosted checkout with no remote therefore matches nothing — it would
/// need its own machine's folders to be known before it could, and that is a
/// larger change than this one.
fn same_repository(project: &Project, repository: &Repository) -> bool {
    match (&project.remote, &repository.remote) {
        (Some(left), Some(right)) => left == right,
        (None, None) => {
            repository.host.is_none()
                && project.common_dir.is_some()
                && project.common_dir == repository.common_dir
        },
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
        // A checkout on another machine makes no project. Everything a project
        // row holds — `root`, `common_dir` — is a path, and a path from over
        // there written into a row here would name a folder on this machine that
        // is either missing or, worse, somebody else's. Attributing a remote
        // agent to a project that already exists is this change's job;
        // *creating* one from a remote checkout is deliberately not, and this is
        // the single place that makes that hold however the caller behaves.
        .filter(|repository| repository.host.is_none())
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
        let root = Location::local(&repository.root);
        if let Some(name) = for_cwd(projects, &root)
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
        if for_cwd(projects, &root).is_some() {
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

    // ----------------------------------------------------------------- rows

    use crate::store::{self, Backend};
    use zellij_tile::prelude::FetchRequest;

    fn backend() -> store::Local {
        let backend = store::Local::shared("hocus_store");
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
        let repository = repository("/work/hocus", "/work/hocus/.git", Some("main"));
        assert_eq!(repository.checkout_label().as_deref(), Some("hocus/main"));
    }

    /// The case the label exists for. The worktree's own folder is `topic`, which
    /// says nothing about which repository it belongs to — and this repository
    /// keeps its worktrees *inside* the checkout, so the folder above is not the
    /// repository either.
    #[test]
    fn a_linked_worktree_is_named_by_the_repository_it_came_from() {
        let repository = repository(
            "/work/hocus/.worktrees/topic",
            "/work/hocus/.git",
            Some("topic"),
        );
        assert_eq!(repository.checkout_label().as_deref(), Some("hocus/topic"));
    }

    /// A slash is legal in a branch name and the label keeps it whole: `feature/x`
    /// is one branch, not a repository called `feature`.
    #[test]
    fn a_slashed_branch_survives_intact() {
        let repository = repository("/work/hocus", "/work/hocus/.git", Some("feature/nested"));
        assert_eq!(
            repository.checkout_label().as_deref(),
            Some("hocus/feature/nested")
        );
    }

    /// A detached HEAD is on no branch. The repository alone is the honest answer;
    /// printing the folder in the branch's place would claim something false.
    #[test]
    fn a_detached_head_is_named_by_its_repository_alone() {
        let repository = repository("/work/hocus/.worktrees/topic", "/work/hocus/.git", None);
        assert_eq!(repository.checkout_label().as_deref(), Some("hocus"));
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
            common_dir: Some("/srv/git/hocus.git".to_owned()),
            branch: Some("topic".to_owned()),
            ..Repository::default()
        };
        assert_eq!(repository.checkout_label().as_deref(), Some("hocus/topic"));
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
        assert_eq!(repository.checkout_label().as_deref(), Some("topic/topic"));
    }

    #[test]
    fn a_project_survives_a_round_trip_through_the_store() {
        let backend = backend();
        let project = checkout("hocus", "/work/hocus");
        backend.apply(vec![to_mutation(&project)]).expect("written");
        assert_eq!(
            read_back(&backend),
            Registry::from([("hocus".to_owned(), project)])
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
            .apply(vec![to_mutation(&checkout("hocus", "/work/hocus"))])
            .expect("written");

        let plain = validate("hocus", "/work/hocus").unwrap();
        backend.apply(vec![to_mutation(&plain)]).expect("written");

        let stored = &read_back(&backend)["hocus"];
        assert_eq!(stored.remote, None);
        assert_eq!(stored.worktree_status, None);
        assert_eq!(stored.common_dir, None);
        assert!(!stored.discovered);
    }

    #[test]
    fn a_diff_that_changes_nothing_writes_nothing() {
        let registry = Registry::from([("hocus".to_owned(), checkout("hocus", "/work/hocus"))]);
        assert!(diff_mutations(&registry, &registry).is_empty());
    }

    #[test]
    fn a_diff_writes_only_the_projects_that_changed() {
        let before = Registry::from([
            (
                "hocus".to_owned(),
                validate("hocus", "/work/hocus").unwrap(),
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
                "hocus".to_owned(),
                validate("hocus", "/work/hocus").unwrap(),
            ),
            (
                "notes".to_owned(),
                validate("notes", "/work/notes").unwrap(),
            ),
        ]);
        let after = Registry::from([("hocus".to_owned(), before["hocus"].clone())]);

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
        for (name, root) in [("hocus", "/work/hocus"), ("notes", "/work/notes")] {
            after.insert(name.to_owned(), validate(name, root).unwrap());
        }
        backend
            .apply(diff_mutations(&before, &after))
            .expect("applied");
        assert_eq!(read_back(&backend), after);

        // And the removal half, against rows that are really there.
        let before = read_back(&backend);
        let after = Registry::from([("hocus".to_owned(), before["hocus"].clone())]);
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
        let old = validate("hocus", "/work/hocus").unwrap();
        backend.apply(vec![to_mutation(&old)]).expect("written");

        let new = validate("hocus-again", "/work/hocus").unwrap();
        backend
            .apply(vec![delete_mutation("hocus"), to_mutation(&new)])
            .expect("renamed");

        let stored = read_back(&backend);
        assert_eq!(stored.len(), 1);
        assert!(stored.contains_key("hocus-again"));
    }

    /// Discovery is unchanged: it still takes a `&mut Registry`, and the rows
    /// are reached by materializing before and diffing after.
    #[test]
    fn discovery_runs_against_a_materialized_registry() {
        let backend = backend();
        let repositories = [Repository {
            root: "/work/hocus".to_owned(),
            remote: Some("git@example.com:me/hocus".to_owned()),
            worktree_status: Some(WorktreeStatus::Primary),
            common_dir: Some("/work/hocus/.git".to_owned()),
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
        assert!(validate("hocus", "/work/hocus").is_ok());
        assert!(validate("", "/work/hocus").is_err());
        assert!(validate("hocus", "work/hocus").is_err());
        assert!(validate("hocus", "/work/../hocus").is_err());
    }

    /// The probe reports a root without a last separator. A user who typed one
    /// must still name the same folder, or the two are separate registry keys
    /// and `discover` makes a second project for a checkout that already has one.
    #[test]
    fn validation_takes_a_last_separator_off_a_root() {
        assert_eq!(
            validate("hocus", "/work/hocus/").unwrap().root,
            "/work/hocus"
        );
        assert_eq!(
            validate("hocus", "/work//hocus").unwrap().root,
            "/work/hocus"
        );
    }

    #[test]
    fn folder_matching_stops_at_component_boundaries() {
        let project = validate("hocus", "/work/hocus").unwrap();
        assert!(contains(&project, &Location::local("/work/hocus")));
        assert!(contains(&project, &Location::local("/work/hocus/src")));
        assert!(!contains(&project, &Location::local("/work/hocus-old")));
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
        assert_eq!(
            for_cwd(&projects, &Location::local("/work/hocus/src"))
                .unwrap()
                .name,
            "hocus"
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
        assert_eq!(
            for_cwd(&projects, &Location::local("/work/hocus/src"))
                .unwrap()
                .name,
            "hocus"
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
        let projects = parse_legacy(br#"{"hocus":{"name":"hocus","root":"/work/hocus"}}"#);
        assert!(!projects["hocus"].discovered);
    }

    /// Discovery used to make a project out of any agent folder. Those saves
    /// go away on load; the ones the user named stay, repository or not.
    #[test]
    fn a_saved_folder_project_from_discovery_is_dropped_on_load() {
        let projects = parse_legacy(
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
        let saved = parse_legacy(json(&projects).unwrap().as_bytes());

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

    // -- checkouts on another machine ------------------------------------

    /// The headline case, and the reason any of this exists: a build box has the
    /// same layout as the laptop, so a remote agent's folder spells a path a
    /// local project owns. Matching it by that path files the agent under work
    /// it is not doing.
    #[test]
    fn a_folder_on_another_machine_is_not_inside_a_local_project() {
        let project = validate("hocus", "/work/hocus").unwrap();
        let projects = registry(std::slice::from_ref(&project));
        let there = Location::new(Some("build-box"), "/work/hocus/src");

        assert!(!contains(&project, &there));
        assert_eq!(for_cwd(&projects, &there), None);
    }

    /// What a remote checkout *may* match on: the one identity that means the
    /// same thing on both machines.
    #[test]
    fn a_remote_checkout_joins_the_project_its_remote_url_names() {
        let mut project = validate("hocus", "/work/hocus").unwrap();
        project.remote = Some("git@example.com:me/hocus".to_owned());
        let projects = registry(&[project]);

        let there = Repository {
            root: "/srv/hocus".to_owned(),
            remote: Some("git@example.com:me/hocus".to_owned()),
            worktree_status: Some(WorktreeStatus::Primary),
            common_dir: Some("/srv/hocus/.git".to_owned()),
            ..Repository::default()
        }
        .on_host(Some("build-box".to_owned()));

        assert_eq!(
            for_repository(&projects, &there).map(|project| project.name.as_str()),
            Some("hocus")
        );
    }

    /// A `common_dir` is a bare path with nothing in it to say whose filesystem
    /// it is. Two machines that both keep their checkout at the same place would
    /// otherwise look like one repository.
    #[test]
    fn a_remote_checkout_with_no_remote_url_matches_nothing() {
        let mut project = validate("hocus", "/work/hocus").unwrap();
        project.common_dir = Some("/work/hocus/.git".to_owned());
        let projects = registry(&[project]);

        let there = Repository {
            root: "/work/hocus".to_owned(),
            worktree_status: Some(WorktreeStatus::Primary),
            common_dir: Some("/work/hocus/.git".to_owned()),
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
            git("/work/hocus", None, WorktreeStatus::Primary).on_host(Some("build-box".to_owned()));

        let discovery = discover(&mut projects, &[there]);

        assert!(discovery.added.is_empty(), "made a project: {discovery:?}");
        assert!(projects.is_empty(), "{projects:?}");
    }

    fn git(root: &str, remote: Option<&str>, status: WorktreeStatus) -> Repository {
        Repository {
            root: root.to_owned(),
            remote: remote.map(str::to_owned),
            worktree_status: Some(status),
            common_dir: Some("/repos/hocus/.git".to_owned()),
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
        let primary = git("/work/hocus", None, WorktreeStatus::Primary);
        let nested = git(
            "/work/hocus/.claude/worktrees/topic",
            None,
            WorktreeStatus::Linked,
        );

        discover(&mut projects, &[primary.clone(), nested.clone()]);

        assert_eq!(projects.len(), 1, "one project: {projects:?}");
        let project = projects.values().next().expect("a project");
        assert_eq!(project.root, "/work/hocus");
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
