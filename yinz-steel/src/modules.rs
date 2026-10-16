//! Confined source loading. Parsing and import rewriting run inside the interpreter worker.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};
use steel::parser::{ast::ExprKind, parser::Parser, tokens::TokenType};

const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
const MAX_MODULES: usize = 1024;
const MAX_IMPORT_DEPTH: usize = 64;

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleSource {
    pub id: String,
    pub source: String,
}

enum Root {
    File { directory: PathBuf, handle: File },
    Embedded(BTreeMap<PathBuf, String>),
}

pub struct SourceRoot {
    root: Root,
    entry: String,
    location: Option<yinz_types::input::layout::RunPluginLocation>,
    paths: BTreeMap<String, PathBuf>,
    loaded: BTreeSet<String>,
    bytes: usize,
}

impl SourceRoot {
    /// An absent entry leaf is allowed for editor checks; its parent must exist inside the root.
    pub fn file(entry: &Path, root: Option<&Path>) -> Result<Self> {
        extension(entry)?;
        let absolute = if entry.is_absolute() {
            entry.to_owned()
        } else {
            std::env::current_dir()
                .context("resolve plugin working directory")?
                .join(entry)
        };
        let parent = absolute
            .parent()
            .context("plugin has no parent directory")?;
        let root = root
            .unwrap_or(parent)
            .canonicalize()
            .context("resolve plugin source root")?;
        let entry = match fs::symlink_metadata(&absolute) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => parent
                .canonicalize()
                .context("resolve plugin parent")?
                .join(absolute.file_name().context("plugin has no filename")?),
            _ => absolute
                .canonicalize()
                .context("resolve plugin entrypoint")?,
        };
        extension(&entry)?;
        let relative = entry
            .strip_prefix(&root)
            .context("plugin entrypoint escapes its source root")?
            .to_owned();
        let id = module_id(&relative)?;
        use std::os::unix::fs::OpenOptionsExt;
        let handle = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&root)
            .context("open plugin source root")?;
        anyhow::ensure!(
            handle
                .metadata()
                .context("inspect plugin source root")?
                .is_dir(),
            "plugin source root is not a directory"
        );
        Ok(Self {
            root: Root::File {
                directory: root,
                handle,
            },
            entry: id.clone(),
            location: None,
            paths: BTreeMap::from([(id, relative)]),
            loaded: BTreeSet::new(),
            bytes: 0,
        })
    }

    pub fn embedded(entry: &str, sources: BTreeMap<String, String>) -> Result<Self> {
        let entry = clean_relative(Path::new(entry))?;
        extension(&entry)?;
        let mut modules = BTreeMap::new();
        let mut total = 0usize;
        for (name, source) in sources {
            let path = clean_relative(Path::new(&name))?;
            extension(&path)?;
            anyhow::ensure!(
                source.len() <= crate::MAX_SOURCE_BYTES,
                "embedded source exceeds 8 MiB"
            );
            total = total.saturating_add(source.len());
            anyhow::ensure!(
                total <= MAX_GRAPH_BYTES && modules.len() < MAX_MODULES,
                "embedded module graph exceeds limits"
            );
            anyhow::ensure!(
                modules.insert(path, source).is_none(),
                "duplicate embedded module path"
            );
        }
        let id = module_id(&entry)?;
        Ok(Self {
            root: Root::Embedded(modules),
            entry: id.clone(),
            location: None,
            paths: BTreeMap::from([(id, entry)]),
            loaded: BTreeSet::new(),
            bytes: 0,
        })
    }

    pub fn entry_id(&self) -> &str {
        &self.entry
    }

    pub fn with_location(mut self, location: yinz_types::input::layout::RunPluginLocation) -> Self {
        self.location = Some(location);
        self
    }

    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            root: match &self.root {
                Root::File { directory, handle } => Root::File {
                    directory: directory.clone(),
                    handle: handle.try_clone().context("retain plugin source root")?,
                },
                Root::Embedded(sources) => Root::Embedded(sources.clone()),
            },
            entry: self.entry.clone(),
            location: self.location.clone(),
            paths: self.paths.clone(),
            loaded: self.loaded.clone(),
            bytes: self.bytes,
        })
    }

    pub fn location(&self) -> Result<String> {
        self.location_for(
            self.paths
                .get(&self.entry)
                .context("missing entrypoint path")?,
        )
    }

    fn location_for(&self, relative: &Path) -> Result<String> {
        if let Some(yinz_types::input::layout::RunPluginLocation::Git(repository)) = &self.location
        {
            let mut repository = repository.clone();
            repository.entrypoint = relative.to_owned();
            return Ok(repository.descriptor());
        }
        match &self.root {
            Root::File { directory, .. } => Ok(url::Url::from_file_path(directory.join(relative))
                .map_err(|_| anyhow!("invalid plugin file path"))?
                .to_string()),
            Root::Embedded(_) => {
                anyhow::ensure!(
                    relative
                        .extension()
                        .is_some_and(|extension| extension == "scm"),
                    "embedded plugin locations require a .scm entrypoint"
                );
                Ok(format!(
                    "yinz:{}",
                    relative
                        .with_extension("")
                        .to_str()
                        .context("builtin path is not UTF-8")?
                ))
            },
        }
    }

    /// Resolve a Scheme target without evaluating it or granting general filesystem access.
    pub fn resolve(&self, specifier: &str) -> Result<String> {
        let origin = self
            .paths
            .get(&self.entry)
            .context("missing entrypoint path")?;
        self.resolve_from_root(&origin.parent().unwrap_or(Path::new("")).join(specifier))
    }

    /// Validate a root-relative target without using an entrypoint filename as an anchor.
    pub fn resolve_from_root(&self, path: &Path) -> Result<String> {
        anyhow::ensure!(
            !path
                .to_str()
                .context("plugin target is not UTF-8")?
                .contains(['\\', '\0']),
            "invalid plugin relative path"
        );
        extension(path)?;
        let relative = clean_relative(path)?;
        self.check_target(&relative)?;
        self.location_for(&relative)
    }

    fn check_target(&self, relative: &Path) -> Result<()> {
        match &self.root {
            Root::File { handle, .. } => {
                let file =
                    open_beneath(handle, relative).context("resolve confined plugin target")?;
                anyhow::ensure!(
                    file.metadata().context("inspect plugin target")?.is_file(),
                    "plugin target is not a regular file"
                );
            },
            Root::Embedded(sources) => anyhow::ensure!(
                sources.contains_key(relative),
                "embedded plugin target is unavailable"
            ),
        }
        Ok(())
    }

    /// Copy only the entrypoint and its bounded Scheme import graph, retaining original sources.
    pub fn copy_to(
        &mut self,
        destination_root: &Path,
        destination: &Path,
    ) -> Result<std::path::PathBuf> {
        anyhow::ensure!(
            !destination.as_os_str().is_empty()
                && destination
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "plugin copy destination must be a confined relative directory"
        );
        let entry = self
            .paths
            .get(&self.entry)
            .context("missing entrypoint path")?
            .clone();
        self.check_target(&entry)?;
        let source = self.read_entry()?;
        self.begin(source.len())?;
        let origin = self.entry.clone();
        let mut sources = BTreeMap::from([(entry.clone(), source.clone())]);
        prepare(&source, &origin, &mut |origin, specifier| {
            let path = self.paths.get(origin).context("unknown module origin")?;
            let path = clean_relative(&path.parent().unwrap_or(Path::new("")).join(specifier))?;
            self.check_target(&path)?;
            let loaded = self.load(origin, specifier)?;
            sources.insert(path, loaded.source.clone());
            Ok(loaded)
        })?;
        use std::os::unix::fs::OpenOptionsExt;
        let root = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(destination_root)
            .context("open plugin copy root")?;
        let directory = create_directories_beneath(&root, destination, true)?;
        for (relative, source) in sources {
            use std::io::Write;
            use std::os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            };
            let parent = create_directories_beneath(
                &directory,
                relative.parent().context("plugin copy parent")?,
                false,
            )?;
            let name = std::ffi::CString::new(
                relative
                    .file_name()
                    .context("plugin copy filename")?
                    .as_bytes(),
            )
            .context("plugin copy filename contains NUL")?;
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CLOEXEC
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            anyhow::ensure!(
                fd >= 0,
                "create copied Scheme source: {}",
                std::io::Error::last_os_error()
            );
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(source.as_bytes())
                .context("write copied Scheme source")?;
        }
        Ok(destination_root.join(destination).join(entry))
    }

    pub fn read_entry(&self) -> Result<String> {
        let path = self
            .paths
            .get(&self.entry)
            .context("missing entrypoint path")?;
        match &self.root {
            Root::File { handle, .. } => read_source(handle, path),
            Root::Embedded(sources) => sources
                .get(path)
                .cloned()
                .context("embedded entrypoint is unavailable"),
        }
    }

    pub(crate) fn begin(&mut self, entry_bytes: usize) -> Result<()> {
        anyhow::ensure!(
            entry_bytes <= crate::MAX_SOURCE_BYTES,
            "Steel source exceeds 8 MiB"
        );
        self.loaded.clear();
        self.loaded.insert(self.entry.clone());
        self.bytes = entry_bytes;
        Ok(())
    }

    pub fn load(&mut self, origin: &str, specifier: &str) -> Result<ModuleSource> {
        anyhow::ensure!(
            (specifier.starts_with("./") || specifier.starts_with("../"))
                && !specifier.contains(['\\', ':', '?', '#'])
                && !specifier.chars().any(char::is_control),
            "module imports must use explicit relative Scheme paths"
        );
        extension(Path::new(specifier))?;
        let origin = self.paths.get(origin).context("unknown module origin")?;
        let relative = clean_relative(&origin.parent().unwrap_or(Path::new("")).join(specifier))?;
        let (relative, source) = match &self.root {
            Root::File { directory, handle } => {
                let canonical = directory
                    .join(&relative)
                    .canonicalize()
                    .context("resolve imported module")?;
                let relative = canonical
                    .strip_prefix(directory)
                    .context("module symlink escapes plugin source root")?
                    .to_owned();
                extension(&relative)?;
                let source = read_source(handle, &relative)?;
                (relative, source)
            },
            Root::Embedded(sources) => (
                relative.clone(),
                sources
                    .get(&relative)
                    .context("embedded module is unavailable")?
                    .clone(),
            ),
        };
        anyhow::ensure!(
            source.len() <= crate::MAX_SOURCE_BYTES,
            "module source exceeds 8 MiB"
        );
        let id = module_id(&relative)?;
        if self.loaded.insert(id.clone()) {
            self.bytes = self.bytes.saturating_add(source.len());
            anyhow::ensure!(
                self.bytes <= MAX_GRAPH_BYTES && self.loaded.len() <= MAX_MODULES,
                "module graph exceeds 64 MiB or 1024 modules"
            );
        }
        self.paths.insert(id.clone(), relative);
        Ok(ModuleSource { id, source })
    }
}

fn read_source(handle: &File, path: &Path) -> Result<String> {
    let file = open_beneath(handle, path).context("open confined module source")?;
    anyhow::ensure!(
        file.metadata().context("inspect module source")?.is_file(),
        "module is not a regular file"
    );
    let mut source = String::new();
    file.take(crate::MAX_SOURCE_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .context("read Scheme source")?;
    anyhow::ensure!(
        source.len() <= crate::MAX_SOURCE_BYTES,
        "module source exceeds 8 MiB"
    );
    Ok(source)
}

fn extension(path: &Path) -> Result<()> {
    anyhow::ensure!(
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("scm" | "ss" | "sld")
        ),
        "module path needs an explicit .scm, .ss or .sld extension"
    );
    Ok(())
}

fn clean_relative(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::CurDir => {},
            Component::ParentDir => {
                anyhow::ensure!(result.pop(), "module path escapes source root")
            },
            _ => bail!("module path must be relative"),
        }
    }
    anyhow::ensure!(!result.as_os_str().is_empty(), "module path is empty");
    Ok(result)
}

fn module_id(path: &Path) -> Result<String> {
    Ok(format!(
        "yinz-module:{}",
        path.to_str().context("module paths must be UTF-8")?
    ))
}

// Pin each directory descriptor so a symlink swap after canonicalization cannot escape the root.
fn open_beneath(root: &File, path: &Path) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut directory = root.try_clone().context("retain plugin source root")?;
    let parts: Vec<_> = path.components().collect();
    for (index, component) in parts.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("noncanonical module path")
        };
        let name = std::ffi::CString::new(name.as_bytes()).context("module path contains NUL")?;
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        if index + 1 != parts.len() {
            flags |= libc::O_DIRECTORY;
        }
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("open module path component");
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

fn create_directories_beneath(root: &File, path: &Path, fresh: bool) -> Result<File> {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
    let mut directory = root.try_clone().context("retain plugin copy root")?;
    let parts: Vec<_> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            bail!("noncanonical plugin copy path")
        };
        let name_c =
            std::ffi::CString::new(name.as_bytes()).context("plugin copy path contains NUL")?;
        if unsafe { libc::mkdirat(directory.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if (fresh && index + 1 == parts.len())
                || error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(error).context("create fresh plugin copy directory");
            }
        }
        directory =
            open_beneath(&directory, Path::new(name)).context("open plugin copy directory")?;
        anyhow::ensure!(
            directory
                .metadata()
                .context("inspect plugin copy directory")?
                .is_dir(),
            "plugin copy path is not a directory"
        );
    }
    Ok(directory)
}

pub(crate) fn prepare(
    source: &str,
    entry: &str,
    loader: &mut impl FnMut(&str, &str) -> Result<ModuleSource>,
) -> Result<(String, BTreeMap<String, String>)> {
    let mut modules = BTreeMap::new();
    let mut seen = BTreeSet::from([entry.to_owned()]);
    let mut bytes = source.len();
    let entry_source = rewrite(
        source,
        entry,
        loader,
        &mut modules,
        &mut seen,
        &mut bytes,
        0,
    )?;
    Ok((entry_source, modules))
}

fn rewrite(
    source: &str,
    origin: &str,
    loader: &mut impl FnMut(&str, &str) -> Result<ModuleSource>,
    modules: &mut BTreeMap<String, String>,
    seen: &mut BTreeSet<String>,
    bytes: &mut usize,
    depth: usize,
) -> Result<String> {
    crate::validate_source(source)?;
    anyhow::ensure!(depth <= MAX_IMPORT_DEPTH, "module import depth exceeds 64");
    let expressions = Parser::new(source, None)
        .without_lowering()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("parse module imports in {origin}: {error:?}"))?;
    let mut imports = Vec::new();
    for expression in &expressions {
        collect_imports(expression, &mut imports)?;
    }
    let mut replacements = Vec::new();
    for (start, end, specifier) in imports {
        let loaded = loader(origin, &specifier)
            .with_context(|| format!("resolve {specifier:?} from {origin}"))?;
        if seen.insert(loaded.id.clone()) {
            *bytes = bytes.saturating_add(loaded.source.len());
            anyhow::ensure!(
                *bytes <= MAX_GRAPH_BYTES && seen.len() <= MAX_MODULES,
                "module graph exceeds limits"
            );
            let rewritten = rewrite(
                &loaded.source,
                &loaded.id,
                loader,
                modules,
                seen,
                bytes,
                depth + 1,
            )?;
            modules.insert(loaded.id.clone(), rewritten);
        }
        replacements.push((
            start,
            end,
            serde_json::to_string(&loaded.id).context("encode virtual module name")?,
        ));
    }
    replacements.sort_by_key(|(start, _, _)| *start);
    let mut rewritten = source.to_owned();
    for (start, end, replacement) in replacements.into_iter().rev() {
        anyhow::ensure!(
            rewritten.is_char_boundary(start)
                && rewritten.is_char_boundary(end)
                && start <= end
                && end <= rewritten.len(),
            "invalid module source span"
        );
        rewritten.replace_range(start..end, &replacement);
    }
    Ok(rewritten)
}

type Import = (usize, usize, String);

fn collect_imports(expression: &ExprKind, imports: &mut Vec<Import>) -> Result<()> {
    match expression {
        ExprKind::List(list) => {
            if list
                .first_ident()
                .is_some_and(|name| name.resolve() == "case")
            {
                if let Some(key) = list.args.get(1) {
                    collect_imports(key, imports)?;
                }
                // Clause heads are literal datums; only the key and clause bodies execute.
                for clause in list.args.iter().skip(2) {
                    if let ExprKind::List(clause) = clause {
                        for expression in clause.args.iter().skip(1) {
                            collect_imports(expression, imports)?;
                        }
                    }
                }
                return Ok(());
            }
            if list
                .first_ident()
                .is_some_and(|name| name.resolve() == "quasiquote")
            {
                for value in &list.args[1..] {
                    collect_quoted_imports(value, 1, imports)?;
                }
                return Ok(());
            }
            if let Some(ExprKind::Atom(head)) = list.args.first() {
                match &head.syn.ty {
                    TokenType::Quote | TokenType::DefineSyntax => return Ok(()),
                    TokenType::Require => {
                        for spec in &list.args[1..] {
                            require_spec(spec, imports)?;
                        }
                        return Ok(());
                    },
                    _ => {},
                }
            }
            for item in &list.args {
                collect_imports(item, imports)?;
            }
        },
        ExprKind::Require(require) => {
            for spec in &require.modules {
                require_spec(spec, imports)?;
            }
        },
        ExprKind::Atom(_) | ExprKind::Quote(_) | ExprKind::Vector(_) => {},
        _ => bail!("unexpected lowered expression while reading module imports"),
    }
    Ok(())
}

fn collect_quoted_imports(
    expression: &ExprKind,
    depth: usize,
    imports: &mut Vec<Import>,
) -> Result<()> {
    if let ExprKind::Quote(quote) = expression {
        return collect_quoted_imports(&quote.expr, depth, imports);
    }
    let (values, head) = match expression {
        ExprKind::List(list) => (&list.args, list.first_ident().map(|name| name.resolve())),
        ExprKind::Vector(vector) => (&vector.args, None),
        _ => return Ok(()),
    };
    let (values, depth) = match head {
        Some("quasiquote") => (&values[1..], depth + 1),
        Some("unquote" | "unquote-splicing" | "#%unquote" | "#%unquote-splicing") => {
            if depth == 1 {
                for value in &values[1..] {
                    collect_imports(value, imports)?;
                }
                return Ok(());
            }
            (&values[1..], depth - 1)
        },
        _ => (values.as_slice(), depth),
    };
    for value in values {
        collect_quoted_imports(value, depth, imports)?;
    }
    Ok(())
}

fn require_spec(expression: &ExprKind, imports: &mut Vec<Import>) -> Result<()> {
    if let Some(path) = expression.string_literal() {
        let span = expression.span();
        imports.push((span.start as usize, span.end as usize, path.to_owned()));
        return Ok(());
    }
    if let ExprKind::List(list) = expression {
        match list.first_ident().map(|s| s.resolve()) {
            Some("only-in") if list.args.len() >= 2 => return require_spec(&list.args[1], imports),
            Some("prefix-in") if list.args.len() == 3 => {
                return require_spec(&list.args[2], imports)
            },
            _ => {},
        }
    }
    Err(anyhow!(
        "require expects a relative Scheme source, only-in, or prefix-in"
    ))
}

#[cfg(test)]
#[path = "unit/modules.rs"]
mod tests;
