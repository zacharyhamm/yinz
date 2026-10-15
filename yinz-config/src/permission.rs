use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use {yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE, yinz_types::data::PermissionType};

pub type GrantedPermission = HashMap<String, Vec<PermissionType>>;

#[derive(Debug)]
enum PermissionChange {
    Replace(Vec<PermissionType>),
    Grant(Vec<PermissionType>),
}

fn extend_permissions(granted: &mut Vec<PermissionType>, permissions: &[PermissionType]) {
    for permission in permissions {
        if !granted.contains(permission) {
            granted.push(*permission);
        }
    }
}

#[derive(Default, Debug)]
pub struct PermissionCache {
    path: PathBuf,
    granted: GrantedPermission,
    pending: HashMap<String, PermissionChange>,
}

impl PermissionCache {
    pub fn cache(&mut self, plugin_name: String, permissions: Vec<PermissionType>) {
        self.granted
            .insert(plugin_name.clone(), permissions.clone());
        self.pending
            .insert(plugin_name, PermissionChange::Replace(permissions));
    }

    /// Add these permissions to the grants present when the cache is saved.
    pub fn grant(&mut self, plugin_name: String, permissions: Vec<PermissionType>) {
        let mut granted = self
            .get_permissions(plugin_name.clone())
            .cloned()
            .unwrap_or_default();
        extend_permissions(&mut granted, &permissions);
        self.granted.insert(plugin_name.clone(), granted);
        let change = self
            .pending
            .entry(plugin_name)
            .or_insert_with(|| PermissionChange::Grant(vec![]));
        let (PermissionChange::Replace(pending) | PermissionChange::Grant(pending)) = change;
        extend_permissions(pending, &permissions);
    }

    pub fn get_permissions(&self, plugin_name: String) -> Option<&Vec<PermissionType>> {
        Self::permissions_in(&self.granted, &plugin_name)
    }

    fn permissions_in<'a>(
        granted: &'a GrantedPermission,
        plugin_name: &str,
    ) -> Option<&'a Vec<PermissionType>> {
        granted.get(plugin_name).or_else(|| {
            let cwd = std::env::current_dir().ok()?;
            let normalize = |name: &str| {
                let location =
                    yinz_runtime::plugin_location::parse_plugin_location(name, Some(cwd.clone()))
                        .ok();
                match location {
                    Some(yinz_types::input::layout::RunPluginLocation::File(path)) => {
                        Some(yinz_runtime::common_path::source_path(&path, &cwd))
                    },
                    None if !name.contains(':') => Some(yinz_runtime::common_path::source_path(
                        Path::new(name),
                        &cwd,
                    )),
                    _ => None,
                }
            };
            let wanted = normalize(plugin_name)?;
            granted
                .iter()
                .find(|(key, _)| normalize(key).as_ref() == Some(&wanted))
                .map(|(_, grants)| grants)
        })
    }

    pub fn check_permissions(
        &self,
        plugin_name: String,
        permissions_to_check: &Vec<PermissionType>,
    ) -> bool {
        if let Some(target) = self.get_permissions(plugin_name) {
            let mut all_granted = true;
            for permission in permissions_to_check {
                if !target.contains(permission) {
                    all_granted = false;
                }
            }
            return all_granted;
        }

        false
    }

    pub fn from_path_or_default(cache_path: Option<PathBuf>) -> Self {
        let cache_path = cache_path.unwrap_or(YINZ_PLUGIN_PERMISSIONS_CACHE.to_path_buf());

        let granted = match fs::read_to_string(cache_path.clone()) {
            Ok(raw_string) => PermissionCache::from_string(raw_string).unwrap_or_default(),
            Err(e) => {
                log::error!("Failed to read permission cache file: {}", e);
                GrantedPermission::default()
            },
        };

        PermissionCache {
            path: cache_path,
            granted,
            pending: HashMap::new(),
        }
    }

    pub fn write_to_file(&mut self) -> std::io::Result<()> {
        // Each server reads its own snapshot. Merge only this writer's changes under a
        // stable lock, so saving a grant cannot restore another server's revoked grant.
        let path = match self.path.canonicalize() {
            Ok(path) => path,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && !self.path.is_symlink() =>
            {
                self.path.clone()
            },
            Err(error) => return Err(error),
        };
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_added_extension("lock"))?;
        lock.lock()?;
        let mut granted = match fs::read_to_string(&path) {
            Ok(contents) => Self::from_string(contents).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => GrantedPermission::new(),
            Err(error) => return Err(error),
        };
        for (plugin_name, change) in &self.pending {
            if let PermissionChange::Replace(permissions) = change {
                granted.insert(plugin_name.clone(), permissions.clone());
            }
        }
        for (plugin_name, change) in &self.pending {
            if let PermissionChange::Grant(permissions) = change {
                // Migrate legacy grants from the locked disk snapshot after replacements,
                // so neither concurrent nor same-write revocations can be undone.
                let mut current = Self::permissions_in(&granted, plugin_name)
                    .cloned()
                    .unwrap_or_default();
                extend_permissions(&mut current, permissions);
                granted.insert(plugin_name.clone(), current);
            }
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        write!(staged, "{}", Self::to_string(&granted))?;
        staged.as_file().sync_all()?;
        staged.persist(path)?;
        self.granted = granted;
        self.pending.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit/permission_cache_tests.rs"]
mod tests;
