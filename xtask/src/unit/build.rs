use super::*;

#[test]
fn requested_profiles_map_to_cargo_artifact_directories() {
    for (release, explicit, expected, directory) in [
        (false, None, "dev", "debug"),
        (true, None, "release", "release"),
        (false, Some("dev"), "dev", "debug"),
        (false, Some("custom"), "custom", "custom"),
    ] {
        let flags = flags::Build {
            release,
            profile: explicit.map(str::to_owned),
            args: vec![],
        };
        assert_eq!(profile_of(&flags), expected);
        assert_eq!(profile_dir(expected), directory);
    }
}
