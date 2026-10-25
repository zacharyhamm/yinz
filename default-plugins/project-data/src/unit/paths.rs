use super::*;

#[test]
fn normalization_takes_off_a_last_separator_and_joins_repeated_ones() {
    assert_eq!(normalize("/work/agent/").unwrap(), "/work/agent");
    assert_eq!(normalize("/work//agent").unwrap(), "/work/agent");
    assert_eq!(normalize("/work/agent").unwrap(), "/work/agent");
}

#[test]
fn the_filesystem_root_is_left_alone() {
    assert_eq!(normalize("/").unwrap(), "/");
}

#[test]
fn a_relative_or_traversing_path_is_refused() {
    assert!(normalize("work/agent").is_err());
    assert!(normalize("/work/../agent").is_err());
    assert!(normalize("").is_err());
}

/// `..` has to be refused because resolving it needs the filesystem, but a
/// `.` names the folder it sits in and `Components` drops it for us.
#[test]
fn a_here_segment_is_simply_dropped() {
    assert_eq!(normalize("/work/./agent").unwrap(), "/work/agent");
}
