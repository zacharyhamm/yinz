//! Persistent project names and their root folders.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

const FILE: &str = "/cache/projects.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub root: String,
    /// True when a poll made this project from an agent folder, not the user.
    ///
    /// A saved project from before this field existed has no key, so it loads
    /// as a user project. That is the safe direction: discovery never removes
    /// a user project.
    #[serde(default)]
    pub discovered: bool,
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
            Some(project)
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
}

impl Discovery {
    pub fn changed(&self) -> bool {
        !self.added.is_empty() || !self.replaced.is_empty()
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

/// Make a project for each folder that no project owns yet.
///
/// The folders are sorted with the shortest first, so the result does not
/// change with the order the panes came in. A folder that an existing project
/// owns makes nothing.
pub fn discover(projects: &mut Registry, cwds: &[String]) -> Discovery {
    let mut roots: Vec<&String> = cwds.iter().filter(|cwd| !cwd.trim().is_empty()).collect();
    roots.sort_by_key(|cwd| (Path::new(cwd).components().count(), cwd.as_str()));
    roots.dedup();

    let mut discovery = Discovery::default();
    for root in roots {
        if for_cwd(projects, root).is_some() {
            continue;
        }
        let Some(leaf) = leaf_name(root) else {
            continue;
        };
        let Ok(project) = validate(&leaf, root) else {
            continue;
        };
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
pub fn adopt(projects: &mut Registry, project: Project) -> Option<String> {
    let stale = projects
        .values()
        .find(|other| other.discovered && other.root == project.root && other.name != project.name)
        .map(|other| other.name.clone());
    if let Some(name) = &stale {
        projects.remove(name);
    }
    projects.insert(project.name.clone(), project);
    stale
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

    fn cwds(cwds: &[&str]) -> Vec<String> {
        cwds.iter().map(|cwd| (*cwd).to_owned()).collect()
    }

    fn registry(projects: &[Project]) -> Registry {
        projects
            .iter()
            .map(|project| (project.name.clone(), project.clone()))
            .collect()
    }

    #[test]
    fn discovery_makes_a_project_from_an_unclaimed_folder() {
        let mut projects = Registry::new();
        let discovery = discover(&mut projects, &cwds(&["/work/hocus/src"]));

        assert_eq!(discovery.added, vec!["src".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(projects["src"].root, "/work/hocus/src");
        assert!(projects["src"].discovered);
    }

    #[test]
    fn an_agent_inside_a_known_project_starts_no_new_project() {
        let mut projects = registry(&[validate("hocus", "/work/hocus").unwrap()]);
        let discovery = discover(&mut projects, &cwds(&["/work/hocus/src"]));

        assert!(!discovery.changed());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_keeps_a_name_stable_across_polls() {
        let mut projects = Registry::new();
        discover(&mut projects, &cwds(&["/work/hocus"]));
        let again = discover(&mut projects, &cwds(&["/work/hocus"]));

        assert!(!again.changed());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_dedupes_a_name_that_is_taken() {
        let mut projects = Registry::new();
        let discovery = discover(&mut projects, &cwds(&["/a/hocus", "/b/hocus"]));

        assert_eq!(discovery.added, vec!["hocus".to_owned(), "hocus-2".to_owned()]);
        assert_eq!(projects["hocus"].root, "/a/hocus");
        assert_eq!(projects["hocus-2"].root, "/b/hocus");
    }

    #[test]
    fn a_shallower_root_absorbs_its_discovered_children() {
        let mut projects = Registry::new();
        discover(&mut projects, &cwds(&["/work/hocus/src"]));
        let discovery = discover(&mut projects, &cwds(&["/work/hocus"]));

        assert_eq!(discovery.added, vec!["hocus".to_owned()]);
        assert_eq!(discovery.replaced, vec![("src".to_owned(), "hocus".to_owned())]);
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn one_pass_puts_the_shallowest_folder_first() {
        let mut projects = Registry::new();
        let discovery = discover(&mut projects, &cwds(&["/work/hocus/src", "/work/hocus"]));

        assert_eq!(discovery.added, vec!["hocus".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    }

    #[test]
    fn discovery_never_absorbs_a_user_project() {
        let mut projects = registry(&[validate("hocus", "/work/hocus").unwrap()]);
        let discovery = discover(&mut projects, &cwds(&["/work"]));

        assert_eq!(discovery.added, vec!["work".to_owned()]);
        assert!(discovery.replaced.is_empty());
        assert_eq!(for_cwd(&projects, "/work/hocus/src").unwrap().name, "hocus");
        assert_eq!(for_cwd(&projects, "/work/other").unwrap().name, "work");
    }

    #[test]
    fn a_user_project_replaces_a_discovered_one_on_the_same_root() {
        let mut projects = Registry::new();
        discover(&mut projects, &cwds(&["/work/hocus"]));
        let stale = adopt(&mut projects, validate("pocus", "/work/hocus").unwrap());

        assert_eq!(stale, Some("hocus".to_owned()));
        assert_eq!(projects.keys().collect::<Vec<_>>(), vec!["pocus"]);
        assert!(!projects["pocus"].discovered);
    }

    #[test]
    fn discovery_skips_the_filesystem_root() {
        let mut projects = Registry::new();
        let discovery = discover(&mut projects, &cwds(&["/", ""]));

        assert!(!discovery.changed());
        assert!(projects.is_empty());
    }

    #[test]
    fn saved_projects_without_the_flag_load_as_user_projects() {
        let projects = parse(br#"{"hocus":{"name":"hocus","root":"/work/hocus"}}"#);
        assert!(!projects["hocus"].discovered);
    }

    #[test]
    fn the_discovered_flag_survives_a_save_and_a_load() {
        let mut projects = Registry::new();
        discover(&mut projects, &cwds(&["/work/hocus"]));
        let saved = parse(json(&projects).unwrap().as_bytes());

        assert_eq!(saved, projects);
        assert!(saved["hocus"].discovered);
    }
}
