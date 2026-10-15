use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn service_modules_compile_in_the_confined_graph() {
    let (_, _) = plugin_with_source("agent-service", "(require \"./agent-service/state.scm\") (require \"./agent-service/probes.scm\") (require \"./agent-service/messages.scm\") (require \"./agent-service/runs.scm\") (require \"./agent-service/observe.scm\") (require \"./agent-service/summaries.scm\") (require \"./agent-service/completions.scm\") (require \"./agent-service/operations.scm\")", json!({}), BTreeMap::new(), "");
}
