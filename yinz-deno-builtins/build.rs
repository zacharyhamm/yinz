//! `include_dir!` tells Cargo about the files it embeds but not about their folder, so a new
//! builtin's `<tag>.ts` would not rebuild this crate and the plugin host would link a stale copy
//! that reports it as not compiled in. Watching the folder reruns this script, and so rebuilds
//! the crate, whenever a file in it is added, removed or edited.
fn main() {
    println!("cargo::rerun-if-changed=assets/plugins");
}
