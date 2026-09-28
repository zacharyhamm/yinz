use super::*;

#[test]
fn shared_packages_resolve_the_same_features_in_both_graphs() {
    // Before the fix, nix's `ioctl` came only from yinz-server-os, which the suite lacks. syn
    // is built twice in each graph, for the proc macros and for the rest, in listing order that
    // differs; yinz-server-os and insta are each in one graph only.
    let binary = "\
yinz v0.45.0 (/w) []
nix v0.30.1 [fs,ioctl,term]
yinz-runtime v0.45.0 (/w/yinz-runtime) []
nix v0.30.1 [fs,ioctl,term] (*)
serde_derive v1.0.228 (proc-macro) [default]
syn v2.0.119 [derive]
syn v2.0.119 [derive,full]
yinz-server-os v0.45.0 (/w/yinz-server-os) []
";
    let suite = "\
yinz-integration-tests v0.45.0 (/w/yinz-integration-tests) []
yinz-runtime v0.45.0 (/w/yinz-runtime) []
nix v0.30.1 [fs,term]
serde_derive v1.0.228 (proc-macro) [default] (*)
syn v2.0.119 [derive,full]
syn v2.0.119 [derive] (*)
insta v1.48.0 [default]
";
    assert_eq!(
        diverging_features(binary, suite),
        ["  nix v0.30.1: binary [fs,ioctl,term], suite [fs,term]"]
    );

    let binary = binary.replace("[fs,ioctl,term]", "[fs,term]");
    assert_eq!(diverging_features(&binary, suite), Vec::<String>::new());

    // A feature set only one graph builds is a build the other cannot reuse.
    let suite = suite.replace("syn v2.0.119 [derive] (*)", "syn v2.0.119 [full]");
    assert_eq!(
        diverging_features(&binary, &suite),
        ["  syn v2.0.119: binary [derive] [derive,full], suite [derive,full] [full]"]
    );
}
