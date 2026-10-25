use super::*;
use hegel::generators::{self, Generator};

/// A path built from generated components, absolute or relative.
#[hegel::composite]
fn paths(tc: &hegel::TestCase) -> PathBuf {
    let absolute = tc.draw(generators::booleans());
    let components: Vec<String> = tc.draw(
        generators::vecs(
            generators::text()
                .min_size(1)
                .filter(|s: &String| !s.contains(std::path::MAIN_SEPARATOR) && !s.contains('\0')),
        )
        .min_size(1),
    );
    let mut path = PathBuf::from(if absolute { "/" } else { "" });
    for component in components {
        path.push(component);
    }
    path
}

/// True when `prefix` is a component-wise prefix of `path`.
fn is_component_prefix(prefix: &Path, path: &Path) -> bool {
    let mut path_components = path.components();
    prefix
        .components()
        .all(|c| path_components.next() == Some(c))
}

#[hegel::test]
fn common_path_all_is_a_prefix_of_every_input(tc: hegel::TestCase) {
    let all: Vec<PathBuf> = tc.draw(generators::vecs(paths()).min_size(1));
    let Some(common) = common_path_all(all.iter().map(|p| p.as_path())) else {
        return;
    };
    for path in &all {
        assert!(
            is_component_prefix(&common, path),
            "{:?} is not a component-wise prefix of {:?} (all: {:?})",
            common,
            path,
            all
        );
    }
}

#[hegel::test]
fn common_path_is_symmetric(tc: hegel::TestCase) {
    let one = tc.draw(paths());
    let two = tc.draw(paths());
    assert_eq!(
        common_path(&one, &two),
        common_path(&two, &one),
        "common_path is not symmetric for {:?} and {:?}",
        one,
        two
    );
}

#[hegel::test]
fn common_path_all_is_order_independent(tc: hegel::TestCase) {
    let all: Vec<PathBuf> = tc.draw(generators::vecs(paths()).min_size(1));
    // a drawn permutation of the same paths
    let mut remaining: Vec<PathBuf> = all.clone();
    let mut shuffled: Vec<PathBuf> = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let idx = tc.draw(generators::integers::<usize>().max_value(remaining.len() - 1));
        shuffled.push(remaining.remove(idx));
    }
    assert_eq!(
        common_path_all(all.iter().map(|p| p.as_path())),
        common_path_all(shuffled.iter().map(|p| p.as_path())),
        "common_path_all depends on order: {:?} vs {:?}",
        all,
        shuffled
    );
}

#[test]
fn compare_all_paths() {
    let one = Path::new("/foo/bar/baz/one.txt");
    let two = Path::new("/foo/bar/quux/quuux/two.txt");
    let three = Path::new("/foo/bar/baz/foo/bar");
    let result = Path::new("/foo/bar");
    let path_permutations = vec![
        vec![one, two, three],
        vec![one, three, two],
        vec![two, one, three],
        vec![two, three, one],
        vec![three, one, two],
        vec![three, two, one],
    ];
    for all in path_permutations {
        assert_eq!(common_path_all(all).unwrap(), result.to_path_buf())
    }
}

#[test]
fn compare_paths() {
    let one = Path::new("/foo/bar/baz/one.txt");
    let two = Path::new("/foo/bar/quux/quuux/two.txt");
    let result = Path::new("/foo/bar");
    assert_eq!(common_path(one, two).unwrap(), result.to_path_buf())
}

#[test]
fn no_common_path() {
    let one = Path::new("/foo/bar");
    let two = Path::new("./baz/quux");
    assert!(common_path(one, two).is_none());
}
