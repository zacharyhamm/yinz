//! A folder, and the machine it is on.
//!
//! Every map of folders in agent — the probed repositories, the daemon's
//! in-flight probes — used to be keyed by path alone. That was right while every
//! pane's process ran here. A remote pane's folder is a path on *another*
//! machine, so `/home/me/work` names two different places at once, and a
//! path-keyed map silently merges them: one machine's checkout answers for the
//! other's, and an agent is filed under a project it has never been in.
//!
//! So a folder is a path *and* a host. `None` is this machine, which is every
//! pane unless it was opened with `--remote-host`.

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};
use std::path::Path;

/// A folder on a machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    /// The machine the folder is on, or `None` for this one.
    pub host: Option<String>,
    pub path: String,
}

impl Location {
    /// A folder on this machine.
    pub fn local(path: impl AsRef<str>) -> Self {
        Location::new(None::<String>, path)
    }

    /// A folder on `host`, or on this machine when `host` is `None`.
    ///
    /// The path is normalized here, exactly as every other folder taken from
    /// outside is, so `/work/agent/` and `/work/agent` cannot become two keys.
    /// A path that will not normalize is kept verbatim: it is still the best
    /// name we have for wherever the pane is, and refusing it would deny the
    /// pane a folder rather than give it an imperfect one.
    pub fn new(host: Option<impl Into<String>>, path: impl AsRef<str>) -> Self {
        let path = path.as_ref();
        Location {
            host: host.map(Into::into),
            path: crate::paths::normalize(path).unwrap_or_else(|_| path.to_owned()),
        }
    }

    /// The one string that names this folder, for use as a map key.
    ///
    /// A local folder is its own path, byte for byte — which is what it was
    /// before hosts existed, so nothing about a session with no remote panes
    /// changes shape. A hosted folder is `@host:path`.
    ///
    /// `@` rather than the `//host/path` that would read more like a URL,
    /// because that form is not disjoint from a path: `Path` collapses a leading
    /// `//` to a single root, so `//build-box/work/x` *starts with*
    /// `/build-box/work`, and a local pane in a directory named after a machine
    /// would inherit that machine's checkout. `normalize` guarantees every local
    /// key begins with `/`, so the `@` namespace cannot collide with one.
    pub fn key(&self) -> String {
        match &self.host {
            Some(host) => format!("@{host}:{}", self.path),
            None => self.path.clone(),
        }
    }

    /// The inverse of [`Location::key`]. Infallible: anything that is not a
    /// hosted key is a local path, which is also what an older peer would send.
    ///
    /// The split is at the first `:` that *begins the path* — the first `":/"` —
    /// rather than at the first or the last `:` anywhere. Both of those are
    /// wrong on real input: a host may carry a port (`host:22`), so the first
    /// one truncates the host, and a Unix filename may contain a colon
    /// (`/work/a:b`), so the last one swallows most of the path into it. A
    /// normalized path always starts with `/`, which is what makes `":/"` the
    /// boundary.
    pub fn parse(key: &str) -> Self {
        let local = || Location {
            host: None,
            path: key.to_owned(),
        };
        let Some(rest) = key.strip_prefix('@') else {
            return local();
        };
        // A key whose path would not normalize has no `/` to find. Falling back
        // to the first colon keeps such a key hosted — a host and a bad path is
        // closer to the truth than silently calling the whole thing local.
        let Some((host, path)) = rest
            .find(":/")
            .map(|at| (&rest[..at], &rest[at + 1..]))
            .or_else(|| rest.split_once(':'))
        else {
            return local();
        };
        Location {
            host: Some(host.to_owned()),
            path: path.to_owned(),
        }
    }

    /// True when this folder is `other`, or is inside it, **on the same
    /// machine**.
    ///
    /// The host is compared first and the paths component-wise — the same rule
    /// `projects::contains` puts on a project root, so `/work/agent-old` is not
    /// below `/work/agent`. Never compare the keys themselves: they are one
    /// string, and prefix-matching two of them would let a path stand in for a
    /// host.
    pub fn starts_with(&self, other: &Location) -> bool {
        self.host == other.host && Path::new(&self.path).starts_with(&other.path)
    }

    /// How deep this folder is, for picking the most specific of several
    /// matches.
    pub fn depth(&self) -> usize {
        Path::new(&self.path).components().count()
    }
}

/// Serialized as its key, because that is what it has to be: `PollReply` carries
/// a map of these and JSON has no map key but a string. Round-tripping through
/// `key`/`parse` also means a local folder is on the wire exactly as it was
/// before this type existed.
impl Serialize for Location {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.key())
    }
}

impl<'de> Deserialize<'de> for Location {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Ok(Location::parse(&key))
    }
}

#[cfg(test)]
#[path = "unit/location.rs"]
mod tests;
