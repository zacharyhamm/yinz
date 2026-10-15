//! Expected provider data for native planner/restart tests. The shipped implementation is
//! Scheme and has its own plugin registration and integration tests.
pub(crate) fn register_opencode() {
    use yinz_types::agent::provider::{register, AgentProvider};
    let provider: AgentProvider = serde_json::from_value(serde_json::json!({
        "kind":"opencode", "executable":"opencode",
        "launch": {
            "model_args":["--model","{value}"],
            "prompt_args":["--prompt","{value}"],
            "resume_args":["--session","{value}"],
            "resume_last_args":["--continue"],
            "permissions":{"auto":["--auto"],"skip":["--auto"]}
        },
        "restart": {
            "values":["-m","--model","--agent","--log-level","--port","--hostname","--mdns-domain","--replay-limit"],
            "switches":["--auto","--print-logs","--pure","--mdns","--mini","--no-replay"],
            "inline_values":["--cors"],
            "forbidden_commands":["completion","acp","mcp","attach","run","debug","providers","auth","agent","upgrade","uninstall","serve","web","models","stats","export","import","github","pr","session","plugin","plug","db","help"],
            "remove_values":["--prompt","-s","--session"],
            "remove_switches":["-c","--continue","--fork"],
            "keep_positionals":true,"max_positionals":1
        }
    })).unwrap();
    register(0, provider).unwrap();
}
