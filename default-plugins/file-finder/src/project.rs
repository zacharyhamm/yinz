//! What the finder pane's title calls the tree being searched.
//!
//! The label is read straight off the filesystem through the `/host` mount rather than
//! asked of `git`: the plugin has no synchronous way to run a command, and an async
//! `run_command` round trip would mean opening the pane with a placeholder title and
//! correcting it a moment later. Walking for `.git` is what `git rev-parse` does anyway,
//! so the two agree for every checkout that does not move the goalposts with `GIT_DIR` or
//! a ceiling directory. The title is cosmetic, so where they could disagree this reports
//! nothing rather than something wrong.

use std::path::{Path, PathBuf};

/// How the pane title names the tree the finder is searching.
///
/// A git checkout is named by its own folder and the branch it has out — the folder rather
/// than the repository, because two worktrees of one repository are exactly the case a
/// title has to tell apart, and the branch for the same reason. Anywhere else it is just
/// the folder fzf starts in.
///
/// `None` when there is nothing worth saying: a cwd with no final component (`/`), or the
/// relative `.` that [`super::focused_cwd`] falls back to when the host cannot say where
/// the focused pane is.
pub fn label(host: &Path, cwd: &Path) -> Option<String> {
    if !cwd.is_absolute() {
        return None;
    }
    match git_root(host, cwd) {
        Some(root) => {
            let name = leaf(&root)?;
            match branch(host, &root) {
                Some(branch) => Some(format!("{name} ({branch})")),
                None => Some(name),
            }
        },
        None => leaf(cwd),
    }
}

/// The nearest ancestor of `cwd` — itself included — that holds a `.git`.
///
/// `.git` is a directory in an ordinary checkout and a file in a linked worktree, and both
/// mark a root, so this only asks whether the entry exists.
fn git_root(host: &Path, cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| host_path(host, &dir.join(".git")).exists())
        .map(Path::to_path_buf)
}

/// The branch checked out at `root`, or `None` on a detached HEAD or an unreadable one.
fn branch(host: &Path, root: &Path) -> Option<String> {
    let git_dir = git_dir(host, root)?;
    let head = std::fs::read_to_string(host_path(host, &git_dir.join("HEAD"))).ok()?;
    branch_from_head(&head)
}

/// Where `root` keeps its HEAD.
///
/// A plain checkout keeps it in `.git`. A linked worktree's `.git` is a file pointing at
/// the worktree's own directory under the primary checkout's `.git/worktrees/`, which is
/// what makes the branch reported here the worktree's own rather than the primary's.
fn git_dir(host: &Path, root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let hosted = host_path(host, &dot_git);
    if hosted.is_dir() {
        return Some(dot_git);
    }
    let pointer = std::fs::read_to_string(hosted).ok()?;
    let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
    if target.as_os_str().is_empty() {
        return None;
    }
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    })
}

/// Read a branch name out of the contents of a `HEAD` file.
///
/// A detached HEAD holds a commit rather than a name. Naming it would put a hash in the
/// title where the reader expects a branch, and the folder alone is the honest answer.
fn branch_from_head(head: &str) -> Option<String> {
    let reference = head.trim().strip_prefix("ref:")?.trim();
    let branch = reference.strip_prefix("refs/heads/")?;
    (!branch.is_empty()).then(|| branch.to_owned())
}

/// The final component of a path, as something printable.
fn leaf(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().into_owned())
}

/// An absolute path as this plugin sees it, through the `/host` mount.
///
/// `host` is a parameter rather than the constant `/host` so the tests can point the walk
/// at a temporary tree.
fn host_path(host: &Path, path: &Path) -> PathBuf {
    host.join(path.strip_prefix("/").unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A fake `/host`: paths are absolute as the plugin sees them, and land inside the
    /// temporary directory the way the real mount lands on the real filesystem root.
    struct Host {
        dir: TempDir,
    }

    impl Host {
        fn new() -> Self {
            Host {
                dir: TempDir::new().expect("a temporary directory"),
            }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        /// Create the directory `path` names, as seen from inside the mount.
        fn dir(&self, path: &str) -> PathBuf {
            fs::create_dir_all(host_path(self.root(), Path::new(path))).expect("a directory");
            PathBuf::from(path)
        }

        fn file(&self, path: &str, contents: &str) {
            let hosted = host_path(self.root(), Path::new(path));
            fs::create_dir_all(hosted.parent().expect("a parent")).expect("a directory");
            fs::write(hosted, contents).expect("a file");
        }

        fn label(&self, cwd: &Path) -> Option<String> {
            super::label(self.root(), cwd)
        }
    }

    #[test]
    fn a_checkout_is_named_by_its_folder_and_its_branch() {
        let host = Host::new();
        let repo = host.dir("/work/yinz");
        host.file("/work/yinz/.git/HEAD", "ref: refs/heads/main\n");
        assert_eq!(host.label(&repo), Some("yinz (main)".to_owned()));
    }

    #[test]
    fn a_subdirectory_is_named_by_the_root_above_it() {
        let host = Host::new();
        let inner = host.dir("/work/yinz/yinz-server/src");
        host.file("/work/yinz/.git/HEAD", "ref: refs/heads/main\n");
        assert_eq!(host.label(&inner), Some("yinz (main)".to_owned()));
    }

    /// The case the branch is in the title for: the folder alone would read as a stray
    /// directory, and the repository name alone would be the same string in both trees.
    #[test]
    fn a_linked_worktree_reports_its_own_branch() {
        let host = Host::new();
        host.file("/work/yinz/.git/HEAD", "ref: refs/heads/main\n");
        host.file(
            "/work/yinz/.git/worktrees/finder/HEAD",
            "ref: refs/heads/worktree-file-finder-project-title\n",
        );
        let worktree = host.dir("/work/yinz/.claude/worktrees/finder");
        host.file(
            "/work/yinz/.claude/worktrees/finder/.git",
            "gitdir: /work/yinz/.git/worktrees/finder\n",
        );
        assert_eq!(
            host.label(&worktree),
            Some("finder (worktree-file-finder-project-title)".to_owned())
        );
    }

    #[test]
    fn a_relative_gitdir_resolves_against_the_worktree() {
        let host = Host::new();
        host.file(
            "/work/yinz/.git/worktrees/finder/HEAD",
            "ref: refs/heads/x\n",
        );
        let worktree = host.dir("/work/yinz/.claude/worktrees/finder");
        host.file(
            "/work/yinz/.claude/worktrees/finder/.git",
            "gitdir: ../../../.git/worktrees/finder\n",
        );
        assert_eq!(host.label(&worktree), Some("finder (x)".to_owned()));
    }

    #[test]
    fn a_checkout_with_an_unreadable_head_is_named_by_its_folder_alone() {
        let host = Host::new();
        let repo = host.dir("/work/yinz");
        host.dir("/work/yinz/.git");
        assert_eq!(host.label(&repo), Some("yinz".to_owned()));
    }

    #[test]
    fn a_detached_head_names_no_branch() {
        let host = Host::new();
        let repo = host.dir("/work/yinz");
        host.file(
            "/work/yinz/.git/HEAD",
            "186ec55ef0000000000000000000000000000000\n",
        );
        assert_eq!(host.label(&repo), Some("yinz".to_owned()));
    }

    #[test]
    fn a_folder_outside_a_checkout_is_named_by_itself() {
        let host = Host::new();
        let folder = host.dir("/home/zach/Downloads");
        assert_eq!(host.label(&folder), Some("Downloads".to_owned()));
    }

    /// Neither has a final component to print, and `.` is what the plugin falls back to
    /// when the host cannot say where the focused pane is — a title of "Find file — ." is
    /// worse than no suffix at all.
    #[test]
    fn a_rootless_or_relative_cwd_has_no_label() {
        let host = Host::new();
        assert_eq!(host.label(Path::new("/")), None);
        assert_eq!(host.label(Path::new(".")), None);
    }

    #[test]
    fn head_names_the_branch_it_points_at() {
        assert_eq!(
            branch_from_head("ref: refs/heads/main\n"),
            Some("main".to_owned())
        );
    }

    /// Slashes are legal in a branch name and only the `refs/heads/` prefix is a prefix.
    #[test]
    fn a_slashed_branch_survives_intact() {
        assert_eq!(
            branch_from_head("ref: refs/heads/feature/nested/thing\n"),
            Some("feature/nested/thing".to_owned())
        );
    }

    #[test]
    fn a_head_that_names_no_branch_is_not_one() {
        // A detached HEAD: a bare commit.
        assert_eq!(
            branch_from_head("186ec55ef0000000000000000000000000000000\n"),
            None
        );
        // A ref that is not a branch — a bisect or a rebase leaves these behind.
        assert_eq!(branch_from_head("ref: refs/bisect/bad\n"), None);
        // And nothing at all.
        assert_eq!(branch_from_head(""), None);
        assert_eq!(branch_from_head("ref: refs/heads/\n"), None);
    }
}
