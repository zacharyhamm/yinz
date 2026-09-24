use super::*;

#[test]
fn swamp_tools_are_deferred_and_only_the_reads_are_available_in_planning() {
    let loaded = BTreeSet::from(["swamp"]);
    let before = specs_for(&BTreeSet::new()).to_string();
    let after = specs_for(&loaded).to_string();
    let read_only_after = read_only_specs_for(&loaded).to_string();
    for name in SWAMP_TOOLS {
        assert_eq!(family_of(name).unwrap().name, "swamp");
        assert!(family_of(name).unwrap().deferred);
        // The loader description mentions the family, not individual tools.
        assert!(!before.contains(name));
        assert!(after.contains(name));
        assert!(load_family(Some("swamp"), false).contains(name));
        let reads = matches!(name, "swamp_issue_search" | "swamp_issue_get");
        assert_eq!(
            load_family(Some("swamp"), true).contains(name),
            reads,
            "{name}"
        );
        assert_eq!(READ_ONLY.contains(&name), reads, "{name}");
        assert_eq!(read_only_after.contains(name), reads, "{name}");
        assert_eq!(needs_confirmation(name), !reads, "{name}");
    }
}
