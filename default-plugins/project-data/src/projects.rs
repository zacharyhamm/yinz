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
    let (mut project, mut mutations) = if root.is_some() {
        let (adopted, mutations) = plan_adopt(rows, project)?;
        (adopted.project, mutations)
    } else if worktree_dir.is_some() {
        let mutations = vec![
            assert_projects(rows, "name", &[name])?,
            to_mutation(&project),
        ];
        (project, mutations)
    } else {
        return Ok((project, vec![]));
    };
    if let Some(dir) = worktree_dir {
        project.worktree_dir = (!dir.is_empty()).then(|| dir.to_owned());
        mutations.push(to_mutation(&project));
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
    if source.root != project.root {
        if projects
            .values()
            .any(|other| other.name != source.name && other.root == source.root)
        {
            return Err(
                "another project shares the source root; its todos and notes cannot be separated"
                    .into(),
            );
        }
        mutations.push(assert_projects(rows, "root", &[&source.root])?);
        mutations.extend(crate::todos::plan_move_root(&source.root, &project.root)?);
        mutations.extend(crate::notes::plan_move_root(&source.root, &project.root)?);
    }
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
    let before = from_rows(rows);
    let mut after = before.clone();
    let mut adopted = adopt(&mut after, project);
    let name = adopted.project.name.clone();
    let mut names = vec![name.as_str()];
    if let Some(name) = &adopted.replaced {
        record_replacement(rows, &before[name], &mut adopted.project)?;
        after.insert(adopted.project.name.clone(), adopted.project.clone());
        names.push(name);
    }
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
mod tests {
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
    fn merging_two_completed_lists_keeps_all_1200_items_usable() {
        use crate::todos;
        let backend = backend();
        let (source, target) = merge_projects(&backend);
        let ids = backend
            .apply(
                [&source.root, &target.root]
                    .into_iter()
                    .flat_map(|root| {
                        (0..600).map(move |i| {
                            Mutation::put("todo")
                                .set("root", root.clone())
                                .set("text", format!("task {i}"))
                                .set("done", true)
                                .build()
                        })
                    })
                    .collect(),
            )
            .unwrap()
            .ids;
        backend
            .apply(
                plan_merge(&project_rows(&backend), &source.name, &target.name)
                    .unwrap()
                    .mutations,
            )
            .unwrap();
        let list = todos::fetch_list(&backend, &target.root).unwrap();
        assert_eq!(todos::counts(&list), (0, 1200));
        assert!(todos::fetch_list(&backend, &source.root)
            .unwrap()
            .items
            .is_empty());
        let mut edits = Vec::new();
        for id in &ids {
            assert!(todos::plan_delete(&list, &target.root, id.get() as u64, false).is_ok());
            edits.extend(
                todos::plan_edit(&list, id.get() as u64, "edited", None)
                    .unwrap()
                    .0,
            );
        }
        backend.apply(edits).unwrap();
        let list = todos::fetch_list(&backend, &target.root).unwrap();
        assert!(list.items.iter().all(|item| item.text == "edited"));
        backend
            .apply(
                todos::plan_add(&list, &target.root, "another", None)
                    .unwrap()
                    .0,
            )
            .unwrap();
        let list = todos::fetch_list(&backend, &target.root).unwrap();
        assert_eq!(todos::counts(&list), (1, 1201));
        // Remove items on both sides of the page boundary through the public planner.
        for index in [0, 999, 1199] {
            let list = todos::fetch_list(&backend, &target.root).unwrap();
            backend
                .apply(
                    todos::plan_delete(&list, &target.root, ids[index].get() as u64, false)
                        .unwrap()
                        .0,
                )
                .unwrap();
        }
        assert_eq!(
            todos::counts(&todos::fetch_list(&backend, &target.root).unwrap()),
            (1, 1198)
        );
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
    fn merge_preserves_target_settings_todo_hierarchy_and_note_links() {
        use crate::{notes, todos};
        let backend = backend();
        let (source, mut expected) = merge_projects(&backend);
        let add_todo = |root: &str, parent| {
            let list = todos::fetch_list(&backend, root).unwrap();
            let (mutations, _) = todos::plan_add(&list, root, "task", parent).unwrap();
            backend.apply(mutations).unwrap().ids[0].get() as u64
        };
        let parent = add_todo(&source.root, None);
        let child = add_todo(&source.root, Some(parent));
        let kept = add_todo(&expected.root, None);
        let (mutations, draft) = notes::plan_add(
            0,
            Some(&source.root),
            "note",
            "body",
            "user",
            &["tag".into()],
            &[child],
        )
        .unwrap();
        let note = backend.apply(mutations).unwrap().ids[0].get() as u64;
        backend
            .apply(notes::plan_attach(&backend, note, &draft.tags, &draft.todo_ids).unwrap())
            .unwrap();
        let before_note = notes::fetch_note(&backend, note).unwrap().unwrap();

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
        assert!(todos::fetch_list(&backend, &source.root)
            .unwrap()
            .items
            .is_empty());
        let list = todos::fetch_list(&backend, &expected.root).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(todos::item_of(&list, parent).unwrap().children[0].id, child);
        assert!(todos::item_of(&list, kept).is_some());
        let after_note = notes::fetch_note(&backend, note).unwrap().unwrap();
        assert_eq!(after_note.root.as_deref(), Some(expected.root.as_str()));
        assert_eq!(after_note.id, before_note.id);
        assert_eq!(after_note.tags, before_note.tags);
        assert_eq!(after_note.todo_ids, before_note.todo_ids);
        assert_eq!(after_note.body, before_note.body);

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
    fn merge_requires_distinct_matching_origins_and_exclusive_source_root() {
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
        assert!(plan_merge(&project_rows(&backend), "duplicate", "kept")
            .unwrap_err()
            .contains("shares"));
        target.root = source.root.clone();
        backend.apply(vec![to_mutation(&target)]).unwrap();
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
    fn project_changes_reject_stale_rows_new_names_and_new_root_sharers_atomically() {
        for conflict in [
            "source",
            "target",
            "root sharer",
            "destination",
            "deleted source",
        ] {
            let backend = backend();
            let (source, _) = merge_projects(&backend);
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
                "root sharer" => to_mutation(&validated("shared", &source.root)),
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
}

#[cfg(test)]
#[path = "unit/project_providers.rs"]
mod provider_tests;
