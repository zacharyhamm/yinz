//! A launch's source colors, independent of the destination theme and terminal cells.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SNAPSHOT_VERSION: u32 = 1;
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024;
pub const EVENT_VERSION: u32 = 1;

/// The Codex release whose loader, palettes and terminal derivations the descriptors follow.
/// A macro rather than a constant so descriptor paths and provenance can embed it.
#[macro_export]
macro_rules! codex_verified_version {
    () => {
        "0.162.1"
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentThemeLaunch {
    pub launch_id: String,
    pub snapshot: AgentThemeSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentThemeEvent {
    pub version: u32,
    pub launch_id: String,
    /// `None` ends this launch. An end for any other launch is stale.
    pub snapshot: Option<AgentThemeSnapshot>,
}

impl AgentThemeEvent {
    pub fn valid(&self) -> bool {
        self.version == EVENT_VERSION
            && !self.launch_id.is_empty()
            && self.launch_id.len() <= 128
            && self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.version == SNAPSHOT_VERSION)
    }

    pub fn apply(&self, active: &mut Option<AgentThemeLaunch>) {
        if !self.valid() {
            return;
        }
        if let Some(snapshot) = &self.snapshot {
            if active
                .as_ref()
                .is_none_or(|a| a.launch_id != self.launch_id)
            {
                *active = Some(AgentThemeLaunch {
                    launch_id: self.launch_id.clone(),
                    snapshot: snapshot.clone(),
                });
            }
        } else if active
            .as_ref()
            .is_some_and(|a| a.launch_id == self.launch_id)
        {
            *active = None;
        }
    }
}

/// Offsets refer to the ANSI bytes produced in the same serialization pass.
/// A null palette explicitly restores shell/unmapped output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteAssociation {
    pub offset: usize,
    pub palette: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffLaunch {
    pub launch_id: String,
    pub palette: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentThemeHandoff {
    pub version: u32,
    pub snapshots: Vec<AgentThemeSnapshot>,
    pub associations: Vec<PaletteAssociation>,
    pub active: Option<HandoffLaunch>,
}

impl Default for AgentThemeHandoff {
    fn default() -> Self {
        Self {
            version: 1,
            snapshots: vec![],
            associations: vec![],
            active: None,
        }
    }
}

impl AgentThemeHandoff {
    pub fn intern(&mut self, snapshot: &AgentThemeSnapshot) -> usize {
        if let Some(index) = self.snapshots.iter().position(|s| s == snapshot) {
            return index;
        }
        self.snapshots.push(snapshot.clone());
        self.snapshots.len() - 1
    }

    pub fn associate(&mut self, offset: usize, snapshot: Option<&AgentThemeSnapshot>) {
        let palette = snapshot.map(|s| self.intern(s));
        if self
            .associations
            .last()
            .is_none_or(|a| a.palette != palette)
        {
            self.associations
                .push(PaletteAssociation { offset, palette });
        }
    }

    pub fn valid(&self, contents: &str) -> bool {
        self.version == 1
            && self.snapshots.iter().all(|s| s.version == SNAPSHOT_VERSION)
            && self.associations.first().is_none_or(|a| a.offset == 0)
            && self
                .associations
                .windows(2)
                .all(|a| a[0].offset < a[1].offset)
            && self.associations.iter().all(|a| {
                contents.is_char_boundary(a.offset)
                    && a.palette.is_none_or(|i| i < self.snapshots.len())
            })
            && self.active.as_ref().is_none_or(|a| {
                !a.launch_id.is_empty()
                    && a.launch_id.len() <= 128
                    && a.palette < self.snapshots.len()
            })
    }

    /// Drop a prefix of content and its associations together, retaining the owner at the cut.
    pub fn trim_prefix(&mut self, cut: usize) {
        let owner = self
            .associations
            .iter()
            .rev()
            .find(|a| a.offset <= cut)
            .and_then(|a| a.palette);
        self.associations.retain(|a| a.offset > cut);
        for association in &mut self.associations {
            association.offset -= cut;
        }
        self.associations.insert(
            0,
            PaletteAssociation {
                offset: 0,
                palette: owner,
            },
        );
        // Dropped history must not keep its snapshot table entries alive or consume the budget.
        let old = std::mem::take(&mut self.snapshots);
        let mut indices = BTreeMap::new();
        for index in self
            .associations
            .iter_mut()
            .filter_map(|a| a.palette.as_mut())
            .chain(self.active.iter_mut().map(|a| &mut a.palette))
        {
            *index = *indices.entry(*index).or_insert_with(|| {
                self.snapshots.push(old[*index].clone());
                self.snapshots.len() - 1
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentThemeMapping {
    #[default]
    Semantic,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SourceColor {
    Default,
    Index(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Channel {
    Foreground,
    Background,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    Foreground,
    Background,
    Muted,
    Subtle,
    Accent,
    Success,
    Error,
    Warning,
    SelectionForeground,
    SelectionBackground,
    Surface,
    AdditionBackground,
    RemovalBackground,
    AdditionDimmed,
    RemovalDimmed,
    AdditionWord,
    RemovalWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Target {
    pub role: Role,
    #[serde(default)]
    pub shimmer: bool,
}

impl From<Role> for Target {
    fn from(role: Role) -> Self {
        Self {
            role,
            shimmer: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub token: String,
    pub channel: Channel,
    pub color: SourceColor,
    /// A provider-defined foreground/background combination, in original application colors.
    pub pair: Option<(SourceColor, SourceColor)>,
    /// None means a known, deliberately unmapped token. It participates in collisions.
    pub target: Option<Target>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatchKey {
    pub channel: Channel,
    pub color: SourceColor,
    pub pair: Option<(SourceColor, SourceColor)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collision {
    pub key: MatchKey,
    pub tokens: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available,
    Partial(String),
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentThemeSnapshot {
    pub version: u32,
    pub provider: String,
    pub provider_version: String,
    pub theme: String,
    /// Descriptor revision and theme source paths only; never configuration or environment contents.
    pub provenance: Vec<String>,
    pub availability: Availability,
    pub definitions: Vec<Definition>,
    pub collisions: Vec<Collision>,
}

impl AgentThemeSnapshot {
    /// Decisions compare expressions, never the RGB values of a destination theme.
    /// BTreeMap/BTreeSet keep diagnostics and decisions independent of file order.
    pub fn compile(&self) -> (BTreeMap<MatchKey, Option<Target>>, Vec<Collision>) {
        let mut groups: BTreeMap<MatchKey, Vec<&Definition>> = BTreeMap::new();
        if self.version != SNAPSHOT_VERSION {
            return (BTreeMap::new(), Vec::new());
        }
        for definition in &self.definitions {
            let key = MatchKey {
                channel: definition.channel,
                color: definition.color,
                pair: definition.pair,
            };
            groups.entry(key.clone()).or_default().push(definition);
            if key.pair.is_some() {
                groups
                    .entry(MatchKey { pair: None, ..key })
                    .or_default()
                    .push(definition);
            }
        }
        let mut collisions = Vec::new();
        let mapping = groups
            .into_iter()
            .map(|(key, definitions)| {
                let targets: BTreeSet<_> = definitions.iter().map(|d| d.target).collect();
                let target = if targets.len() == 1 {
                    targets.first().copied().flatten()
                } else {
                    collisions.push(Collision {
                        key: key.clone(),
                        tokens: definitions
                            .iter()
                            .map(|d| d.token.clone())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect(),
                    });
                    if matches!(self.provider.as_str(), "codex" | "claude") {
                        // Shared UI, syntax and decorative colors follow semantic roles.
                        // Prefer Codex's explicit accent, then the stable Role order
                        // (base text before accent before status colors).
                        definitions
                            .iter()
                            .filter_map(|definition| {
                                definition
                                    .target
                                    .map(|target| (definition.token != "codex.accent", target))
                            })
                            .min()
                            .map(|(_, target)| target)
                    } else {
                        None
                    }
                };
                (key, target)
            })
            .collect();
        (mapping, collisions)
    }

    pub fn finish(&mut self) {
        self.collisions = self.compile().1;
    }
}

#[cfg(test)]
#[path = "unit/theme_tests.rs"]
mod tests;

impl std::fmt::Display for AgentThemeMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Semantic => "semantic",
            Self::Off => "off",
        })
    }
}
impl std::str::FromStr for AgentThemeMapping {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "semantic" => Ok(Self::Semantic),
            "off" => Ok(Self::Off),
            _ => Err("agent_theme_mapping must be semantic or off".into()),
        }
    }
}
