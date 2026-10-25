use super::*;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn forwarding_uses_only_the_bound_routing_environment() {
    for (token, remote) in [
        (None, false),
        (Some(""), false),
        (Some("bound-token"), true),
    ] {
        let mut env = HashMap::from([("YINZ_PANE_REF".into(), "bound-pane".into())]);
        if let Some(token) = token {
            env.insert("YINZ_REMOTE_PANE_TOKEN".into(), token.into());
        }
        let command = forwarding_command(Path::new("yinz"), "codex", &env);
        let args: Vec<_> = command.get_args().collect();
        let expected: &[&str] = if remote {
            &["--pty-broker", "report", "codex"]
        } else {
            &["action", "agent-report", "--hook-json", "--kind", "codex"]
        };
        assert_eq!(args, expected.iter().map(OsStr::new).collect::<Vec<_>>());
        let changes: HashMap<_, _> = command.get_envs().collect();
        for key in ROUTE_ENV {
            assert_eq!(
                changes.get(OsStr::new(key)).copied(),
                Some(env.get(*key).map(OsStr::new))
            );
        }
    }
}
