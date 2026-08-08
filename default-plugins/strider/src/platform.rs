use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Default for Platform {
    fn default() -> Self {
        Platform::Unix
    }
}

impl Platform {
    /// Detect the host platform from the initial_cwd string.
    /// Checks for drive letter (e.g. `C:\` or `C:/`) or UNC path (`\\server\`).
    pub fn detect(initial_cwd: &str) -> Self {
        let bytes = initial_cwd.as_bytes();
        // Drive letter: X:\ or X:/
        let is_drive_letter = matches!(
            (bytes.get(0), bytes.get(1), bytes.get(2)),
            (Some(c), Some(b':'), Some(b'\\' | b'/')) if c.is_ascii_alphabetic()
        );
        if is_drive_letter {
            return Platform::Windows;
        }
        // UNC path: \\server\share
        let is_unc_path = matches!((bytes.get(0), bytes.get(1)), (Some(b'\\'), Some(b'\\')));
        if is_unc_path {
            return Platform::Windows;
        }
        Platform::Unix
    }

    /// Replace `\` with `/` so WASM PathBuf can parse the path correctly.
    /// No-op for paths that don't contain backslashes.
    pub fn normalize(path: &Path) -> PathBuf {
        let s = path.to_string_lossy();
        if s.contains('\\') {
            PathBuf::from(s.replace('\\', "/"))
        } else {
            path.to_path_buf()
        }
    }

    /// Convert internal forward-slash path back to native backslashes for Windows display.
    /// Identity on Unix.
    pub fn to_host_display(path: &Path, platform: Platform) -> String {
        let s = path.to_string_lossy();
        match platform {
            Platform::Windows => s.replace('/', "\\"),
            Platform::Unix => s.into_owned(),
        }
    }

    /// Path separator character for the host platform.
    pub fn separator(self) -> char {
        match self {
            Platform::Windows => '\\',
            Platform::Unix => '/',
        }
    }

    /// Ensure bare drive letters like `C:` become `C:/`.
    /// WASM PathBuf's `parent()` strips the trailing slash, but on Windows
    /// `C:` means "current directory on drive C", not the drive root.
    pub fn ensure_drive_root(path: PathBuf, platform: Platform) -> PathBuf {
        if platform == Platform::Windows {
            let s = path.to_string_lossy();
            let bytes = s.as_bytes();
            let is_bare_drive = s.len() == 2
                && matches!(bytes.get(0), Some(c) if c.is_ascii_alphabetic())
                && matches!(bytes.get(1), Some(b':'));
            if is_bare_drive {
                return PathBuf::from(format!("{}/", s));
            }
        }
        path
    }

    /// Display name for a virtual root entry.
    /// `C:/` → `C:\`, `//wsl.localhost/Ubuntu/` → `Ubuntu (WSL)`, `/` → `/`.
    pub fn virtual_root_display_name(path: &Path, platform: Platform) -> String {
        let s = path.to_string_lossy();
        match platform {
            Platform::Windows => {
                if s.starts_with("//wsl.localhost/") {
                    let rest = &s["//wsl.localhost/".len()..];
                    let distro = rest.trim_end_matches('/');
                    return format!("{} (WSL)", distro);
                }
                Platform::to_host_display(path, platform)
            },
            Platform::Unix => s.into_owned(),
        }
    }

    /// Check if a path is a filesystem root.
    /// Unix: `/` or empty.
    /// Windows: `X:/` or `X:` (drive root), or `//server/share` (UNC root with <= 4 components).
    pub fn is_root(path: &Path, platform: Platform) -> bool {
        let s = path.to_string_lossy();
        match platform {
            Platform::Unix => s == "/" || s.is_empty(),
            Platform::Windows => {
                let bytes = s.as_bytes();
                // Drive root: "C:/" or "C:"
                let is_drive_root_slash = s.len() == 3
                    && matches!(bytes.get(0), Some(c) if c.is_ascii_alphabetic())
                    && matches!(bytes.get(1), Some(b':'))
                    && matches!(bytes.get(2), Some(b'/'));
                if is_drive_root_slash {
                    return true;
                }
                let is_bare_drive = s.len() == 2
                    && matches!(bytes.get(0), Some(c) if c.is_ascii_alphabetic())
                    && matches!(bytes.get(1), Some(b':'));
                if is_bare_drive {
                    return true;
                }
                // UNC root: //server/share (at most 4 path components)
                if s.starts_with("//") {
                    let without_prefix = &s[2..];
                    let parts: Vec<&str> = without_prefix
                        .split('/')
                        .filter(|p| !p.is_empty())
                        .collect();
                    return parts.len() <= 2;
                }
                s.is_empty()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    /// Full-Unicode path strings, mixed with the shapes this module actually branches on
    /// (`\`, `/`, `//`, `:`, drive letters, UNC/WSL roots, empty).
    #[hegel::composite]
    fn any_path(tc: &hegel::TestCase) -> PathBuf {
        // hegel biases `booleans()` toward `false`, so that branch gets the shapes that matter.
        let free_form = tc.draw(generators::booleans());
        if free_form {
            PathBuf::from(tc.draw(generators::text()))
        } else {
            let s = tc.draw(generators::sampled_from(vec![
                "", "/", "//", "\\", "\\\\", ":", "C:", "C:/", "C:\\", "c:", "C:/Users\\user",
                "//server/share", "//server/share/folder", "//wsl.localhost/Ubuntu/",
                "//wsl.localhost/", "//wsl.localhost", "/home/ünïcødé/🦀", "\\\\wsl.localhost\\Ubuntu",
            ]));
            PathBuf::from(s)
        }
    }

    #[hegel::composite]
    fn any_platform(tc: &hegel::TestCase) -> Platform {
        tc.draw(generators::sampled_from(vec![
            Platform::Unix,
            Platform::Windows,
        ]))
    }

    /// "No-op for paths that don't contain backslashes" -- so normalizing twice must not move.
    #[hegel::test]
    fn normalize_is_idempotent(tc: hegel::TestCase) {
        let path = tc.draw(any_path());
        let once = Platform::normalize(&path);
        let twice = Platform::normalize(&once);
        // compared as strings: `PathBuf`'s `PartialEq` is component-wise and would, for instance,
        // call "C:" and "C:/" equal -- exactly the distinction these helpers exist to make.
        assert_eq!(
            once.to_string_lossy(),
            twice.to_string_lossy(),
            "normalize({:?}) = {:?} but normalizing again gave {:?}",
            path, once, twice
        );
    }

    #[hegel::test]
    fn ensure_drive_root_is_idempotent(tc: hegel::TestCase) {
        let path = tc.draw(any_path());
        let platform = tc.draw(any_platform());
        let once = Platform::ensure_drive_root(path.clone(), platform);
        let twice = Platform::ensure_drive_root(once.clone(), platform);
        assert_eq!(
            once.to_string_lossy(),
            twice.to_string_lossy(),
            "ensure_drive_root({:?}, {:?}) = {:?} but applying it again gave {:?}",
            path, platform, once, twice
        );
    }

    /// "Identity on Unix" -- a path with no backslashes survives normalize + host display.
    #[hegel::test]
    fn normalize_then_display_is_the_identity_on_unix(tc: hegel::TestCase) {
        let path = tc.draw(any_path());
        tc.assume(!path.to_string_lossy().contains('\\'));
        let displayed = Platform::to_host_display(&Platform::normalize(&path), Platform::Unix);
        assert_eq!(
            displayed,
            path.to_string_lossy(),
            "unix round trip of {:?} produced {:?}",
            path,
            displayed
        );
    }

    /// `ensure_drive_root` normalizes a root, it does not demote one. Note that it is *not*
    /// the identity on roots -- rewriting the bare drive `C:` into `C:/` is the whole reason
    /// the function exists -- so the invariant is that a root stays a root.
    // `assume` throws away most draws here, so ask for more of them.
    #[hegel::test(test_cases = 2000, suppress_health_check = [hegel::HealthCheck::FilterTooMuch])]
    fn ensure_drive_root_keeps_roots_rooted(tc: hegel::TestCase) {
        let path = tc.draw(any_path());
        let platform = tc.draw(any_platform());
        tc.assume(Platform::is_root(&path, platform));
        let ensured = Platform::ensure_drive_root(path.clone(), platform);
        assert!(
            Platform::is_root(&ensured, platform),
            "{:?} is a root on {:?} but ensure_drive_root turned it into {:?}, which is not",
            path,
            platform,
            ensured
        );
    }

    /// `virtual_root_display_name` byte-slices its input; it must never panic on any path.
    #[hegel::test]
    fn path_helpers_never_panic(tc: hegel::TestCase) {
        let path = tc.draw(any_path());
        let platform = tc.draw(any_platform());
        let _ = Platform::detect(&path.to_string_lossy());
        let _ = Platform::normalize(&path);
        let _ = Platform::to_host_display(&path, platform);
        let _ = Platform::is_root(&path, platform);
        let _ = Platform::virtual_root_display_name(&path, platform);
    }

    #[test]
    fn detect_unix() {
        assert_eq!(Platform::detect("/home/user"), Platform::Unix);
        assert_eq!(Platform::detect("/"), Platform::Unix);
    }

    #[test]
    fn detect_windows_drive() {
        assert_eq!(Platform::detect("C:\\Users\\user"), Platform::Windows);
        assert_eq!(Platform::detect("D:/Projects"), Platform::Windows);
    }

    #[test]
    fn detect_windows_unc() {
        assert_eq!(
            Platform::detect("\\\\wsl.localhost\\Ubuntu"),
            Platform::Windows
        );
    }

    #[test]
    fn normalize_backslashes() {
        let p = PathBuf::from("C:\\Users\\user");
        assert_eq!(Platform::normalize(&p), PathBuf::from("C:/Users/user"));
    }

    #[test]
    fn normalize_noop_unix() {
        let p = PathBuf::from("/home/user");
        assert_eq!(Platform::normalize(&p), PathBuf::from("/home/user"));
    }

    #[test]
    fn to_host_display_windows() {
        let p = PathBuf::from("C:/Users/user");
        assert_eq!(
            Platform::to_host_display(&p, Platform::Windows),
            "C:\\Users\\user"
        );
    }

    #[test]
    fn to_host_display_unix() {
        let p = PathBuf::from("/home/user");
        assert_eq!(Platform::to_host_display(&p, Platform::Unix), "/home/user");
    }

    #[test]
    fn is_root_unix() {
        assert!(Platform::is_root(&PathBuf::from("/"), Platform::Unix));
        assert!(Platform::is_root(&PathBuf::from(""), Platform::Unix));
        assert!(!Platform::is_root(&PathBuf::from("/home"), Platform::Unix));
    }

    #[test]
    fn is_root_windows_drive() {
        assert!(Platform::is_root(&PathBuf::from("C:/"), Platform::Windows));
        assert!(Platform::is_root(&PathBuf::from("C:"), Platform::Windows));
        assert!(!Platform::is_root(
            &PathBuf::from("C:/Users"),
            Platform::Windows
        ));
    }

    #[test]
    fn ensure_drive_root_fixes_bare_drive() {
        assert_eq!(
            Platform::ensure_drive_root(PathBuf::from("C:"), Platform::Windows),
            PathBuf::from("C:/")
        );
    }

    #[test]
    fn ensure_drive_root_noop_with_slash() {
        assert_eq!(
            Platform::ensure_drive_root(PathBuf::from("C:/"), Platform::Windows),
            PathBuf::from("C:/")
        );
    }

    #[test]
    fn ensure_drive_root_noop_unix() {
        assert_eq!(
            Platform::ensure_drive_root(PathBuf::from("/"), Platform::Unix),
            PathBuf::from("/")
        );
    }

    #[test]
    fn is_root_windows_unc() {
        assert!(Platform::is_root(
            &PathBuf::from("//server/share"),
            Platform::Windows
        ));
        assert!(!Platform::is_root(
            &PathBuf::from("//server/share/folder"),
            Platform::Windows
        ));
    }
}
