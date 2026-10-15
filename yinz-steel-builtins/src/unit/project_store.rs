use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn project_store_modules_compile_in_the_confined_graph() {
    let _ = plugin_with_source("project-store", "(require \"./project-store/ops.scm\" \"./project-store/import.scm\" \"./project-store/arrangement.scm\")", json!({}), BTreeMap::new(), "");
}
