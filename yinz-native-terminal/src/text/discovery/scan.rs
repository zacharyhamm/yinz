use crate::platform::Platform;
use crate::text::font::FaceRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub families: Vec<String>,
    pub monospaced: bool,
    pub weight: u16,
    pub italic: bool,
    /// The OS/2 width class: 1 (ultra-condensed) to 9 (ultra-expanded).
    pub width: u16,
}

/// The OS/2 width class of a face that is neither condensed nor expanded.
pub const NORMAL_WIDTH: u16 = 5;

pub fn preferred_fallbacks(platform: Platform) -> &'static [&'static str] {
    match platform {
        Platform::MacOs => &[
            "Menlo",
            "SF Mono",
            "Monaco",
            "PingFang SC",
            "Hiragino Sans",
            "Apple SD Gothic Neo",
            "Apple Symbols",
            "Apple Color Emoji",
        ],
        Platform::Windows => &[
            "Cascadia Mono",
            "Consolas",
            "Microsoft YaHei",
            "Yu Gothic",
            "Malgun Gothic",
            "Segoe UI Symbol",
            "Segoe UI Emoji",
        ],
        Platform::Linux => &[],
    }
}

pub const EMOJI_FAMILIES: &[&str] = &[
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Noto Color Emoji",
    "Twemoji",
    "JoyPixels",
];

pub fn emoji_order(faces: &[Face]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..faces.len())
        .filter(|index| preference_rank(&faces[*index], EMOJI_FAMILIES) < EMOJI_FAMILIES.len())
        .collect();
    order.sort_by_key(|index| (preference_rank(&faces[*index], EMOJI_FAMILIES), *index));
    order
}

fn named(face: &Face, family: &str) -> bool {
    face.families
        .iter()
        .any(|name| name.eq_ignore_ascii_case(family))
}

const PIVOT_WEIGHT: u16 = 400;

fn weight_distance(weight: u16, target: u16) -> (u16, bool) {
    let distance = weight.abs_diff(target);
    let wrong_side = if target >= PIVOT_WEIGHT {
        weight < target
    } else {
        weight > target
    };
    (distance, wrong_side)
}

/// How far a face is from a request, closest first. Width only breaks ties:
/// a family may ship wider and narrower cuts of each style under one
/// name, and a terminal wants the normal one, but never at the cost
/// of the requested weight or slant.
fn style_distance(face: &Face, request: FaceRequest) -> ((u16, bool), bool, u16) {
    (
        weight_distance(face.weight, request.weight),
        face.italic != request.italic,
        face.width.abs_diff(NORMAL_WIDTH),
    )
}

fn preference_rank(face: &Face, preferred: &[&str]) -> usize {
    preferred
        .iter()
        .position(|family| named(face, family))
        .unwrap_or(preferred.len())
}

pub fn fallback_order(faces: &[Face], preferred: &[&str], request: FaceRequest) -> Vec<usize> {
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by_key(|index| {
        let face = &faces[*index];
        (
            preference_rank(face, preferred),
            !face.monospaced,
            style_distance(face, request),
            *index,
        )
    });
    order
}

pub fn monospace_families(faces: &[Face]) -> Vec<String> {
    let mut families: Vec<String> = faces
        .iter()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().cloned())
        .collect();
    sort_families(&mut families);
    families
}

pub fn sort_families(families: &mut Vec<String>) {
    families.retain(|family| !family.trim().is_empty() && !family.starts_with('.'));
    families.sort_by_key(|family| family.to_lowercase());
    families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
}

pub fn family_choice(faces: &[Face], family: &str, request: FaceRequest) -> Option<usize> {
    (0..faces.len())
        .filter(|index| named(&faces[*index], family))
        .min_by_key(|index| (style_distance(&faces[*index], request), *index))
}

#[cfg(any(target_os = "macos", windows))]
pub use scanner::Scanner;

#[cfg(any(target_os = "macos", windows))]
mod scanner {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use std::collections::HashMap;

    use super::{
        emoji_order, fallback_order, family_choice, monospace_families, preferred_fallbacks, Face,
    };
    use crate::platform::Platform;
    use crate::text::font::FaceRequest;

    pub struct Scanner {
        database: fontdb::Database,
        ids: Vec<fontdb::ID>,
        paths: Vec<PathBuf>,
        faces: Vec<Face>,
        orders: Mutex<HashMap<FaceRequest, Vec<usize>>>,
    }

    impl Scanner {
        pub fn load() -> Option<Mutex<Self>> {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            let scanner = Self::from_database(database);
            if scanner.is_none() {
                report!("no system fonts were found; falling back to the embedded fonts");
            }
            scanner.map(Mutex::new)
        }

        #[cfg(test)]
        pub fn synthetic(directory: &std::path::Path) -> Option<Self> {
            let mut database = fontdb::Database::new();
            database.load_fonts_dir(directory);
            Self::from_database(database)
        }

        fn from_database(database: fontdb::Database) -> Option<Self> {
            let mut ids = Vec::new();
            let mut paths = Vec::new();
            let mut faces = Vec::new();
            for info in database.faces() {
                let path = match &info.source {
                    fontdb::Source::File(path) => path.clone(),
                    fontdb::Source::SharedFile(path, _) => path.clone(),
                    fontdb::Source::Binary(_) => continue,
                };
                ids.push(info.id);
                paths.push(path);
                faces.push(Face {
                    families: info.families.iter().map(|(name, _)| name.clone()).collect(),
                    monospaced: info.monospaced,
                    weight: info.weight.0,
                    italic: info.style != fontdb::Style::Normal,
                    width: info.stretch.to_number(),
                });
            }
            if faces.is_empty() {
                return None;
            }
            Some(Self {
                database,
                ids,
                paths,
                faces,
                orders: Mutex::new(HashMap::new()),
            })
        }

        fn located(&self, index: usize) -> Option<(PathBuf, u32)> {
            let info = self.database.face(self.ids[index])?;
            Some((self.paths[index].clone(), info.index))
        }

        fn covers(&self, index: usize, character: char) -> bool {
            self.database
                .with_face_data(self.ids[index], |data, face| {
                    swash::FontRef::from_index(data, face as usize)
                        .is_some_and(|font| font.charmap().map(character) != 0)
                })
                .unwrap_or(false)
        }

        fn order(&self, request: FaceRequest) -> Vec<usize> {
            let mut orders = match self.orders.lock() {
                Ok(orders) => orders,
                Err(poisoned) => poisoned.into_inner(),
            };
            orders
                .entry(request)
                .or_insert_with(|| {
                    fallback_order(
                        &self.faces,
                        preferred_fallbacks(Platform::current()),
                        request,
                    )
                })
                .clone()
        }

        pub fn match_codepoint(
            &self,
            request: FaceRequest,
            character: char,
        ) -> Option<(PathBuf, u32)> {
            self.order(request)
                .iter()
                .find(|index| self.covers(**index, character))
                .and_then(|index| self.located(*index))
        }

        pub fn match_family(&self, family: &str, request: FaceRequest) -> Option<(PathBuf, u32)> {
            family_choice(&self.faces, family, request).and_then(|index| self.located(index))
        }

        pub fn match_emoji(&self, character: char) -> Option<(PathBuf, u32)> {
            emoji_order(&self.faces)
                .into_iter()
                .find(|index| self.covers(*index, character))
                .and_then(|index| self.located(index))
        }

        pub fn monospace_families(&self) -> Vec<String> {
            monospace_families(&self.faces)
        }
    }
}

#[cfg(test)]
#[path = "unit/scan_tests.rs"]
mod tests;
