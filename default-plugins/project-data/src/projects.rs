//! Persistent projects and the repositories or folders that identify them.

use crate::location::Location;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use yinz_tile::prelude::{DataRevision, DataRowId, DataRows, DataValue, Mutation, Pred};

/// A comparison key only: stored origins and Git configuration keep their spelling.
/// Unknown forms live in a separate key space and compare exactly.
pub fn origin_identity(origin: &str) -> String {
    fn repository(origin: &str) -> Option<String> {
        if origin.chars().any(|c| c.is_whitespace() || c.is_control())
            || origin.contains(['\\', '?', '#'])
        {
            return None;
        }
        let scp;
        let url = if let Some(rest) = origin.strip_prefix("git@") {
            let (host, path) = rest.split_once(':')?;
            if host.is_empty() || host.contains('/') || path.starts_with('/') {
                return None;
            }
            scp = format!("ssh://git@{host}/{path}");
            scp.as_str()
        } else {
            origin
        };
        let (scheme, rest) = url.split_once("://")?;
        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "https" => 443,
            "ssh" => 22,
            _ => return None,
        };
        let (authority, raw_path) = rest.split_once('/')?;
        if authority.is_empty() || authority.ends_with(':') {
            return None;
        }
        let parsed = url::Url::parse(url).ok()?;
        // Do not let URL parsing repair malformed input or resolve path components.
        if parsed.path().strip_prefix('/')? != raw_path {
            return None;
        }
        let path = raw_path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        if path.is_empty()
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return None;
        }
        let host = parsed.host_str()?.to_ascii_lowercase();
        let port = parsed.port().filter(|port| *port != default_port);
        let port = port.map(|port| format!(":{port}")).unwrap_or_default();
        Some(format!("repo:{host}{port}/{path}"))
    }
    repository(origin).unwrap_or_else(|| format!("exact:{origin}"))
}

pub fn equivalent_origins(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((left, right), (Some(left), Some(right))
        if !left.trim().is_empty() && !right.trim().is_empty()
            && origin_identity(left) == origin_identity(right))
}

/// The file the registry lived in before the store. Read once, by the import.
pub const FILE: &str = "/cache/projects.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Host-assigned, provider-qualified identity; absent for Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_identity: Option<String>,
    pub name: String,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_dir: Option<String>,
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
    /// Store row IDs replaced by explicit renames or merges, including their history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_ids: Vec<DataRowId>,
}

impl Project {
    /// True when this project carries a provider or Git checkout identity.
    ///
    /// A user project need not have one — the user can name any folder. A
    /// discovered project always does, because discovery only runs on
    /// checkouts.
    pub fn is_checkout(&self) -> bool {
        self.provider_identity.is_some()
            || self.worktree_status.is_some()
            || self.common_dir.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Primary,
    Linked,
}

/// A resolved project checkout, or an ordinary folder when no provider matched.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// Host-assigned, provider-qualified identity; absent for Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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

    /// True when a custom provider or Git claimed this folder.
    pub fn is_checkout(&self) -> bool {
        self.provider_identity.is_some()
            || self.worktree_status.is_some()
            || self.common_dir.is_some()
    }

    /// `repo:branch` — where this checkout is, in the fewest words that still
    /// tell two worktrees of one repository apart.
    ///
    /// A colon separates the two halves, not a slash: a slash is legal inside a
    /// branch name, so `agent/feature/nested` gives the reader no way to know
    /// where the repository ends. A colon cannot appear in a ref name at all, so
    /// `agent:feature/nested` splits in exactly one place.
    ///
    /// `None` for a folder Git did not claim: there is nothing to say, and a
    /// bare folder name where the reader expects a checkout would read as one.
    ///
    /// The repository half is the *primary* checkout, not this folder, which is
    /// the whole reason it is worth printing: a worktree kept at
    /// `yinz/.worktrees/topic` is named `topic` by its own folder, and that
    /// says nothing about which repository it belongs to. See
    /// [`repository_name`] for how the primary is read out of `common_dir`.
    ///
    /// A detached HEAD is on no branch, so the repository stands alone. Naming
    /// the folder there instead would say "branch" about something that is not
    /// one — the same stance `poll::repository` takes when it refuses to read
    /// Git's `HEAD` as a branch name.
    pub fn checkout_label(&self) -> Option<String> {
        let repository = self.checkout_name()?;
        match &self.branch {
            Some(branch) => Some(format!("{repository}:{branch}")),
            None => Some(repository),
        }
    }

    /// The repository half of [`checkout_label`], with no branch on it.
    ///
    /// What names a *tab*, where the label names a pane. A tab holds work rather
    /// than a checkout — two panes in two worktrees of one repository are one
    /// tab's worth of the same project — so the branch, which is exactly what
    /// tells those two apart, is what has to come off.
    ///
    /// `None` for a folder Git did not claim, for the same reason the label
    /// refuses one: a bare folder name where the reader expects a repository
    /// would read as one.
    pub fn checkout_name(&self) -> Option<String> {
        if !self.is_checkout() {
            return None;
        }
        // A checkout at `/` names no folder, and a lone separator on a pane frame
        // is noise rather than an address.
        self.display_name
            .as_deref()
            .or_else(|| self.common_dir.as_deref().and_then(repository_name))
            .or_else(|| Some(folder_name(&self.root)).filter(|name| !name.is_empty()))
            .map(str::to_owned)
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
            project.worktree_dir = saved.worktree_dir.filter(|dir| !dir.contains('\0'));
            project.provider_identity = saved.provider_identity;
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

        provider_identity: None,
        worktree_dir: None,
        remote: None,
        worktree_status: None,
        common_dir: None,
        discovered: false,
        replaced_ids: Vec::new(),
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
    "worktree_dir",
    "remote",
    "worktree_status",
    "common_dir",
    "discovered",
    "replaced_ids",
    "provider_identity",
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
                    provider_identity: text_at(rows, row, "provider_identity"),
                    worktree_dir: text_at(rows, row, "worktree_dir"),
                    remote: text_at(rows, row, "remote"),
                    worktree_status: text_at(rows, row, "worktree_status")
                        .as_deref()
                        .and_then(|status| status.parse().ok()),
                    common_dir: text_at(rows, row, "common_dir"),
                    discovered: matches!(rows.get(row, "discovered"), Some(DataValue::Bool(true))),
                    replaced_ids: match rows.get(row, "replaced_ids") {
                        Some(DataValue::Text(ids)) => serde_json::from_str(ids).unwrap_or_default(),
                        _ => Vec::new(),
                    },
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
        .set("provider_identity", project.provider_identity.clone())
        .set("name", project.name.clone())
        .set("root", project.root.clone())
        .set("worktree_dir", project.worktree_dir.clone())
        .set("remote", project.remote.clone())
        .set(
            "worktree_status",
            project.worktree_status.map(|s| s.as_str().to_owned()),
        )
        .set("common_dir", project.common_dir.clone())
        .set("discovered", project.discovered)
        .set(
            "replaced_ids",
            serde_json::json!(project.replaced_ids).to_string(),
        )
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
                        .worktree_dir
                        .clone()
                        .map_or(DataValue::Null, DataValue::Text),
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
                    DataValue::Text(serde_json::json!(project.replaced_ids).to_string()),
                    project
                        .provider_identity
                        .clone()
                        .map_or(DataValue::Null, DataValue::Text),
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

/// A rename or merge, ready to apply in one transaction. Publish it only after success.
#[derive(Debug)]
pub struct Change {
    pub replaced: String,
    pub project: Project,
    pub mutations: Vec<Mutation>,
}

/// Guard the matching rows, including their absence. Never plan from a partial registry.
pub fn assert_projects(rows: &DataRows, column: &str, values: &[&str]) -> Result<Mutation, String> {
    if rows.truncated {
        return Err("project list is incomplete; cannot change projects".into());
    }
    let revisions = rows.rows.iter().filter(|row| {
        matches!(rows.get(row, column), Some(DataValue::Text(value)) if values.contains(&value.as_str()))
    }).map(|row| {
        match (rows.get(row, "id"), rows.get(row, "rev")) {
            (Some(DataValue::Int(id)), Some(DataValue::Int(rev))) if *id > 0 && *rev > 0 =>
                Ok((DataRowId::new(*id), DataRevision::new(*rev))),
            _ => Err("project rows are missing revisions; reload and retry".to_owned()),
        }
    }).collect::<Result<Vec<_>, _>>()?;
    Ok(Mutation::AssertRows {
        entity: "project".into(),
        filter: Pred::is_in(column, values.iter().copied()),
        rows: revisions,
    })
}

pub fn plan_rename(rows: &DataRows, name: &str, new_name: &str) -> Result<Change, String> {
    let projects = from_rows(rows);
    let name = name.trim();
    let mut project = projects
        .get(name)
        .cloned()
        .ok_or_else(|| format!("unknown project {name}; call project_list"))?;
    project.name = validate(new_name, &project.root)?.name;
    if projects.contains_key(&project.name) {
        return Err(format!("project {} already exists", project.name));
    }
    project.discovered = false;
    record_replacement(rows, &projects[name], &mut project)?;
    let mutations = vec![
        assert_projects(rows, "name", &[name, &project.name])?,
        to_mutation(&project),
        delete_mutation(name),
    ];
    Ok(Change {
        replaced: name.into(),
        project,
        mutations,
    })
}

/// Remember a project while preserving defaults that the caller did not change.
pub fn plan_remember(
    rows: &DataRows,
    name: &str,
    root: Option<&str>,
    worktree_dir: Option<&str>,
) -> Result<(Project, Vec<Mutation>), String> {
    let name = name.trim();
    let before = from_rows(rows);
    let stored = before.get(name);
    if worktree_dir.is_some_and(|dir| dir.contains('\0')) {
        return Err("worktree_dir must not contain NUL bytes".into());
    }
    let mut project = match root {
        Some(root) => validate(name, root)?,
        None => stored
            .cloned()
            .ok_or_else(|| format!("unknown project {name}; root is required for a new project"))?,
    };
    project.worktree_dir = stored.and_then(|p| p.worktree_dir.clone());
    if root.is_some() {
        let (adopted, mutations) = plan_adopt_with_worktree_dir(rows, project, worktree_dir)?;
        return Ok((adopted.project, mutations));
    }
    let mut mutations = Vec::new();
    if let Some(dir) = worktree_dir {
        mutations.push(assert_projects(rows, "name", &[name])?);
        project.worktree_dir = (!dir.is_empty()).then(|| dir.to_owned());
        if stored != Some(&project) {
            mutations.push(to_mutation(&project));
        }
    }
    Ok((project, mutations))
}

pub fn plan_merge(rows: &DataRows, source: &str, target: &str) -> Result<Change, String> {
    let projects = from_rows(rows);
    let (source, target) = (source.trim(), target.trim());
    if source == target {
        return Err("source and target must be distinct projects".into());
    }
    let lookup = |name: &str| {
        projects
            .get(name)
            .ok_or_else(|| format!("unknown project {name}; call project_list"))
    };
    let source = lookup(source)?;
    let mut project = lookup(target)?.clone();
    if !(source.provider_identity.is_some()
        && source.provider_identity == project.provider_identity)
        && !equivalent_origins(source.remote.as_deref(), project.remote.as_deref())
    {
        return Err(
            if source.provider_identity.is_some() || project.provider_identity.is_some() {
                "projects must have equivalent provider identities"
            } else {
                "projects must have equivalent nonempty origins"
            }
            .into(),
        );
    }
    let mut mutations = vec![assert_projects(rows, "name", &[&source.name, target])?];
    project.discovered = false;
    record_replacement(rows, source, &mut project)?;
    mutations.extend([to_mutation(&project), delete_mutation(&source.name)]);
    Ok(Change {
        replaced: source.name.clone(),
        project,
        mutations,
    })
}

/// The store identity of each name. Recreating a deleted name allocates a new ID.
pub fn row_ids(rows: &DataRows) -> BTreeMap<String, DataRowId> {
    rows.rows
        .iter()
        .filter_map(|row| match (rows.get(row, "name"), rows.get(row, "id")) {
            (Some(DataValue::Text(name)), Some(DataValue::Int(id))) if *id > 0 => {
                Some((name.clone(), DataRowId::new(*id)))
            },
            _ => None,
        })
        .collect()
}

fn record_replacement(
    rows: &DataRows,
    source: &Project,
    target: &mut Project,
) -> Result<(), String> {
    let id = row_ids(rows)
        .get(&source.name)
        .copied()
        .ok_or("project rows are missing IDs; reload and retry")?;
    target.replaced_ids.push(id);
    target.replaced_ids.extend_from_slice(&source.replaced_ids);
    target.replaced_ids.sort_unstable();
    target.replaced_ids.dedup();
    Ok(())
}

/// An explicit adoption and its history are committed together, guarded by the rows read.
pub fn plan_adopt(rows: &DataRows, project: Project) -> Result<(Adopted, Vec<Mutation>), String> {
    plan_adopt_with_worktree_dir(rows, project, None)
}

fn plan_adopt_with_worktree_dir(
    rows: &DataRows,
    project: Project,
    worktree_dir: Option<&str>,
) -> Result<(Adopted, Vec<Mutation>), String> {
    let before = from_rows(rows);
    let mut after = before.clone();
    let mut adopted = adopt(&mut after, project);
    let name = adopted.project.name.clone();
    let mut names = vec![name.as_str()];
    if let Some(name) = &adopted.replaced {
        record_replacement(rows, &before[name], &mut adopted.project)?;
        names.push(name);
    }
    if let Some(dir) = worktree_dir {
        // Apply explicit clearing after adoption inherits the discovered project's defaults.
        adopted.project.worktree_dir = (!dir.is_empty()).then(|| dir.to_owned());
    }
    after.insert(adopted.project.name.clone(), adopted.project.clone());
    let mut mutations = vec![
        assert_projects(rows, "name", &names)?,
        assert_projects(rows, "root", &[&adopted.project.root])?,
    ];
    mutations.extend(diff_mutations(&before, &after));
    Ok((adopted, mutations))
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
    if project.provider_identity.is_some() || repository.provider_identity.is_some() {
        return repository.host.is_none()
            && project.provider_identity.is_some()
            && project.provider_identity == repository.provider_identity;
    }
    match (&project.remote, &repository.remote) {
        (Some(left), Some(right)) => equivalent_origins(Some(left), Some(right)),
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

/// The probed folder map, keyed by folder *and* machine.
///
/// The shape `agent-service` fills from its Git probes and every reader downstream
/// receives on the wire.
pub type Probed = BTreeMap<Location, Repository>;

/// What Git said about a folder, or about the nearest folder above it that we
/// have an answer for.
///
/// The daemon probes each folder a pane is in, so a pane that `cd`s into a
/// subdirectory of its checkout has no answer of its own until a fresh probe
/// lands. It is in the same checkout it was in a moment ago, and on screen the
/// wait reads as the pane losing its project and getting it back. The exact
/// answer replaces this stand-in as soon as it arrives.
///
/// For the project only. A branch takes no stand-in: the folder above may have
/// another one out.
pub fn repository_for_cwd<'a>(probed: &'a Probed, cwd: &Location) -> Option<&'a Repository> {
    if let Some(repository) = probed.get(cwd) {
        return Some(repository);
    }
    probed
        .iter()
        // Component-wise, and on the same machine — the rule
        // `Location::starts_with` puts on a folder, so `/work/agent-old` is
        // not below `/work/agent` and neither is `/work/agent/src` on
        // another host.
        .filter(|(probed, _)| cwd.starts_with(probed))
        .max_by_key(|(probed, _)| probed.depth())
        .map(|(_, repository)| repository)
}

/// The project that owns an identified checkout.
///
/// A folder outside a repository belongs to no project even when a project root
/// contains it: a pane that is not in a checkout is not working on a project.
/// The daemon fills the probed map from its Git probe, so a pane has no project
/// for the moment between it appearing and that probe landing.
///
/// Git identity is asked first and the folder second. What a project *is* is a
/// repository; containment is a proxy for that, and it misfires for a checkout
/// that happens to sit under an unrelated project's root. The containment
/// answer is still worth having second — `same_repository` only matches when
/// both sides carry Git metadata, so a project the user named on a plain folder
/// is found by nothing else, and two projects sharing a remote fall through to
/// the folder rather than to nothing at all.
///
/// For a pane on another machine there is no second answer, and
/// [`contains`] is what says so: a project's root is a path here, and a folder
/// over there that spells the same path is a different place. A remote pane is
/// matched by its repository's remote url or not at all — which is right,
/// because a build box laid out like the laptop would otherwise put every remote
/// pane in whichever local project sits at its path.
pub fn project_for_cwd<'a>(
    probed: &Probed,
    projects: &'a Registry,
    cwd: &Location,
) -> Option<&'a Project> {
    let repository =
        repository_for_cwd(probed, cwd).filter(|repository| repository.is_checkout())?;
    for_repository(projects, repository).or_else(|| {
        for_cwd(projects, cwd).filter(|project| {
            !project.discovered
                || (project.provider_identity.is_none() && repository.provider_identity.is_none())
                || same_repository(project, repository)
        })
    })
}

/// Resolve registered membership, preferring explicit local roots over repository matches.
pub fn registered_for_cwd<'a>(
    probed: &Probed,
    projects: &'a Registry,
    cwd: &Location,
) -> Option<&'a Project> {
    if let Some(project) = projects
        .values()
        .filter(|project| !project.discovered && contains(project, cwd))
        .max_by_key(|project| Path::new(&project.root).components().count())
    {
        return Some(project);
    }
    project_for_cwd(probed, projects, cwd)
}

/// The primary checkout's configuration root, or the supplied project root.
pub fn configuration_root(common: Option<&str>, fallback: &str) -> String {
    common
        .map(Path::new)
        .filter(|path| path.file_name().is_some_and(|name| name == ".git"))
        .and_then(Path::parent)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_owned())
}

/// The arrangement key for a checkout that has no registered project.
pub fn repository_identity(repository: &Repository) -> Option<String> {
    repository
        .provider_identity
        .as_ref()
        .map(|id| format!("provider:{id}"))
        .or_else(|| {
            repository
                .remote
                .as_deref()
                .filter(|remote| !remote.is_empty())
                .map(|remote| format!("remote:{}", origin_identity(remote)))
        })
        .or_else(|| {
            repository
                .common_dir
                .as_ref()
                .map(|dir| format!("git:{:?}:{dir}", repository.host))
        })
}

/// Resolve the identity, label and configuration root used to arrange project panes.
pub fn resolve(
    probed: &Probed,
    projects: &Registry,
    cwd: &Location,
) -> Option<yinz_tile::prelude::ResolvedProject> {
    if let Some(project) = registered_for_cwd(probed, projects, cwd) {
        return Some(yinz_tile::prelude::ResolvedProject {
            identity: format!("project:{}", project.root),
            name: project.name.clone(),
            root: Some(configuration_root(
                project.common_dir.as_deref(),
                &project.root,
            )),
        });
    }
    let repository = repository_for_cwd(probed, cwd).filter(|repo| repo.is_checkout())?;
    Some(yinz_tile::prelude::ResolvedProject {
        identity: repository_identity(repository)?,
        name: repository.checkout_name()?,
        root: repository
            .host
            .is_none()
            .then(|| configuration_root(repository.common_dir.as_deref(), &repository.root)),
    })
}

/// What to *call* the project a folder is in, which is a wider question than
/// which project owns it.
///
/// [`project_for_cwd`] answers with a registered project or with nothing, and
/// nothing is the wrong answer for a tab: a checkout the user never registered
/// is still plainly a place, and naming the tab after it costs nothing and
/// reverts the moment the pane closes. Registering it would not — which is why
/// this is a *label*, and discovery still refuses to remember a folder a shell
/// merely sat in.
///
/// An explicitly registered local root names its folders immediately, even without Git.
/// Otherwise repository matching and the checkout's own name supply the label.
pub fn label_for_cwd(probed: &Probed, projects: &Registry, cwd: &Location) -> Option<String> {
    if let Some(project) = registered_for_cwd(probed, projects, cwd) {
        return Some(project.name.clone());
    }
    repository_for_cwd(probed, cwd)?.checkout_name()
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
/// Only an identified checkout makes a project. An unclaimed folder is
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
            if (project.provider_identity.is_none()
                && project.remote.is_none()
                && project.common_dir.is_none())
                || (project.discovered && project.provider_identity != repository.provider_identity)
            {
                project
                    .provider_identity
                    .clone_from(&repository.provider_identity);
                project.remote.clone_from(&repository.remote);
                project.worktree_status = repository.worktree_status;
                project.common_dir.clone_from(&repository.common_dir);
                if repository.is_checkout() {
                    discovery.updated.push(name);
                }
            }
            continue;
        }
        if for_cwd(projects, &root).is_some_and(|project| {
            !project.discovered
                || (project.provider_identity.is_none() && repository.provider_identity.is_none())
                || same_repository(project, repository)
        }) {
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
                project
                    .provider_identity
                    .clone_from(&repository.provider_identity);
                project.remote.clone_from(&repository.remote);
                project.worktree_status = repository.worktree_status;
                project.common_dir.clone_from(&repository.common_dir);
                discovery.root_moves.push((old, project.root.clone()));
            }
            continue;
        }
        let Some(name) = repository
            .display_name
            .clone()
            .or_else(|| leaf_name(&repository.root))
        else {
            continue;
        };
        let Ok(mut project) = validate(&name, &repository.root) else {
            continue;
        };
        project
            .provider_identity
            .clone_from(&repository.provider_identity);
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
                    && ((below.provider_identity.is_none() && project.provider_identity.is_none())
                        || below.provider_identity == project.provider_identity)
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

/// What `adopt` did: the project as it is now stored, and the discovered
/// name that went away, if one did, so the caller can move the tabs that
/// point at it.
pub struct Adopted {
    pub project: Project,
    pub replaced: Option<String>,
}

/// Add a project the user named.
///
/// A discovered project on the same folder is replaced, preserving its worktree defaults.
pub fn adopt(projects: &mut Registry, mut project: Project) -> Adopted {
    if let Some(stored) = projects.get(&project.name) {
        project.replaced_ids.clone_from(&stored.replaced_ids);
    }
    let replaced = projects
        .values()
        .find(|other| other.discovered && other.root == project.root && other.name != project.name)
        .map(|other| {
            project
                .provider_identity
                .clone_from(&other.provider_identity);
            project.remote.clone_from(&other.remote);
            project.worktree_status = other.worktree_status;
            project.common_dir.clone_from(&other.common_dir);
            if project.worktree_dir.is_none() {
                project.worktree_dir.clone_from(&other.worktree_dir);
            }
            other.name.clone()
        });
    if let Some(name) = &replaced {
        projects.remove(name);
    }
    projects.insert(project.name.clone(), project.clone());
    Adopted { project, replaced }
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
#[path = "unit/projects.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/project_providers.rs"]
mod provider_tests;

#[cfg(test)]
#[path = "unit/project_remember.rs"]
mod remember_tests;
