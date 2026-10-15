use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use url::Url;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitRevision {
    Branch(String),
    Tag(String),
    Commit(String),
}

impl GitRevision {
    pub fn property(&self) -> (&str, &str) {
        match self {
            Self::Branch(value) => ("branch", value),
            Self::Tag(value) => ("tag", value),
            Self::Commit(value) => ("commit", value),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GitPlugin {
    pub repo: String,
    pub entrypoint: PathBuf,
    pub revision: Option<GitRevision>,
}

impl GitPlugin {
    pub fn is_local(&self) -> bool {
        Path::new(&self.repo).is_absolute()
            || self.repo.starts_with("./")
            || self.repo.starts_with("../")
            || self.repo.starts_with("~/")
            || !self.repo.contains(":")
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.repo.is_empty() || self.repo.starts_with('-') || self.repo.contains('\0') {
            return Err("repo must be a nonempty path or Git remote");
        }
        if !self.is_local() {
            if self.repo.contains("://") {
                let url = Url::parse(&self.repo).map_err(|_| "invalid Git remote")?;
                if !matches!(url.scheme(), "https" | "http" | "ssh" | "git")
                    || url.host_str().is_none()
                {
                    return Err("unsupported Git transport");
                }
            } else if self.repo.contains("::")
                || self.repo.split(':').next().is_none_or(str::is_empty)
            {
                return Err("invalid SSH Git remote");
            }
        }
        if self.entrypoint.as_os_str().is_empty()
            || self
                .entrypoint
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || !matches!(
                self.entrypoint.extension().and_then(|ext| ext.to_str()),
                Some("scm" | "ss" | "sld")
            )
        {
            return Err("entrypoint must be a relative Scheme file inside the repository");
        }
        if let Some(revision) = &self.revision {
            if self.is_local() {
                return Err("local repositories cannot select a branch, tag or commit");
            }
            let (_, value) = revision.property();
            if value.is_empty() || value.starts_with('-') || value.contains('\0') {
                return Err("revision must be nonempty");
            }
            if !matches!(revision, GitRevision::Commit(_))
                && (value.contains("..")
                    || value.contains("@{")
                    || value.contains("//")
                    || value
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
                    || value.split('/').any(|part| {
                        part.is_empty()
                            || part.starts_with('.')
                            || part.ends_with('.')
                            || part.ends_with(".lock")
                    }))
            {
                return Err("branch and tag must be valid Git reference names");
            }
            if matches!(revision, GitRevision::Commit(_))
                && (!(7..=64).contains(&value.len())
                    || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err("commit must be a hexadecimal commit ID");
            }
        }
        Ok(())
    }

    pub fn descriptor(&self) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("repo", &self.repo);
        query.append_pair("entrypoint", &self.entrypoint.to_string_lossy());
        if let Some(revision) = &self.revision {
            let (key, value) = revision.property();
            query.append_pair(key, value);
        }
        format!("git://plugin?{}", query.finish())
    }

    pub fn from_descriptor(value: &str) -> Result<Self, &'static str> {
        let url = Url::parse(value).map_err(|_| "invalid Git plugin descriptor")?;
        if url.scheme() != "git"
            || url.host_str() != Some("plugin")
            || url.path() != ""
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err("invalid Git plugin descriptor");
        }
        let mut repo = None;
        let mut entrypoint = None;
        let mut revision = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "repo" if repo.is_none() => repo = Some(value.into_owned()),
                "entrypoint" if entrypoint.is_none() => {
                    entrypoint = Some(PathBuf::from(value.as_ref()))
                },
                "branch" | "tag" | "commit" if revision.is_none() => {
                    revision = Some(match key.as_ref() {
                        "branch" => GitRevision::Branch(value.into_owned()),
                        "tag" => GitRevision::Tag(value.into_owned()),
                        _ => GitRevision::Commit(value.into_owned()),
                    })
                },
                _ => return Err("unknown or duplicate Git plugin property"),
            }
        }
        let plugin = Self {
            repo: repo.ok_or("Git plugin requires repo")?,
            entrypoint: entrypoint.unwrap_or_else(|| Path::new("main.scm").to_owned()),
            revision,
        };
        plugin.validate()?;
        Ok(plugin)
    }
}
