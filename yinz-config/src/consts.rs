use include_dir::{include_dir, Dir};
pub static YINZ_DEFAULT_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/themes");

pub const FEATURES: &[&str] = &[];
