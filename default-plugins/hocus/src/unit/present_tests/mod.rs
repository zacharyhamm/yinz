use super::*;

fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "manual-1".to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

mod commands;
mod escaping;
mod fallbacks;
mod listings;
mod results;
mod tables;
mod verbs;
