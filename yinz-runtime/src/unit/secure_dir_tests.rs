use super::*;

#[test]
fn private_directory_requires_the_effective_owner() {
    let temp = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let directory = File::open(temp.path()).unwrap();
    let owner = directory.metadata().unwrap().uid();
    assert!(validate(&directory, owner, None, true).is_ok());
    assert!(validate(&directory, owner.wrapping_add(1), None, true).is_err());
    assert!(validate(&directory, owner.wrapping_add(1), Some(owner), true).is_err());
}

#[test]
fn unmapped_ancestor_ownership_requires_a_namespace_exception() {
    assert!(trusted_ancestor_owner(0, 1000, None));
    assert!(trusted_ancestor_owner(1000, 1000, None));
    assert!(!trusted_ancestor_owner(65534, 1000, None));
    assert!(trusted_ancestor_owner(65534, 1000, Some(65534)));
    assert!(!trusted_ancestor_owner(1001, 1000, Some(65534)));
}

#[test]
fn unmapped_ancestors_still_require_safe_permissions() {
    let temp = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o755))
        .tempdir()
        .unwrap();
    let directory = File::open(temp.path()).unwrap();
    let owner = directory.metadata().unwrap().uid();
    let uid = owner.wrapping_add(1);
    assert!(validate(&directory, uid, Some(owner), false).is_ok());
    for mode in [0o775, 0o757, 0o777] {
        directory
            .set_permissions(Permissions::from_mode(mode))
            .unwrap();
        assert!(validate(&directory, uid, Some(owner), false).is_err());
    }
    directory
        .set_permissions(Permissions::from_mode(0o1777))
        .unwrap();
    assert!(validate(&directory, uid, Some(owner), false).is_ok());
    assert!(validate(&directory, uid, Some(owner), true).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn namespace_overflow_requires_remapping_and_an_unmapped_overflow_uid() {
    for mapping in ["1000 0 1\n", "0 1000 1\n", "0 1000 1\n1000 2000 100\n"] {
        assert_eq!(namespace_overflow_uid(mapping, "65534\n"), Some(65534));
    }
    assert_eq!(namespace_overflow_uid("1000 0 1", "60000"), Some(60000));
    for mapping in [
        "0 0 4294967295\n",
        "1000 1000 1\n",
        "65534 0 1\n",
        "0 100000 65535\n",
        "1000 0 1\n65534 1001 1\n",
    ] {
        assert_eq!(namespace_overflow_uid(mapping, "65534"), None, "{mapping}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn malformed_namespace_information_does_not_enable_an_exception() {
    for mapping in [
        "",
        "\n",
        "1000 0",
        "1000 0 1 extra",
        "1000 invalid 1",
        "1000 0 0",
        "4294967295 0 1",
        "0 4294967295 1",
        "1000 0 1\ninvalid",
    ] {
        assert_eq!(namespace_overflow_uid(mapping, "65534"), None, "{mapping}");
    }
    for overflow in ["", "invalid", "-1", "4294967296", "65534 extra"] {
        assert_eq!(
            namespace_overflow_uid("1000 0 1", overflow),
            None,
            "{overflow}"
        );
    }
}
