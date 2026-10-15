//! Track directory changes as well as the files embedded by `include_dir!`.
fn main() {
    println!("cargo::rerun-if-changed=assets/plugins");
}
