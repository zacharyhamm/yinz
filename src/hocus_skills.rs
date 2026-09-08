//! The portable skill is part of the binary; installation is explicit.
use std::{fs, io::Write, path::Path};
use zellij_runtime::errors::prelude::*;

const SKILL: &str = include_str!("../assets/skills/zellij-notes-todos/SKILL.md");

pub(crate) fn install(path: &Path, dry_run: bool, force: bool) -> Result<()> {
    match fs::read(path) {
        Ok(bytes) if bytes == SKILL.as_bytes() => {
            println!("Already installed: {}", path.display());
            return Ok(());
        },
        Ok(_) if !force => {
            return Err(anyhow!(
                "{} differs; use --force to replace it",
                path.display()
            ))
        },
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error).context(format!("cannot read {}", path.display())),
    }
    if dry_run {
        println!("Would install: {}", path.display());
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("skill path has no parent"))?;
    fs::create_dir_all(parent).context("cannot create skill directory")?;
    // Write beside the destination so replacement never leaves a partial skill.
    let mut temp = tempfile::NamedTempFile::new_in(parent).context("cannot create skill file")?;
    temp.write_all(SKILL.as_bytes())
        .context("cannot write skill")?;
    if force {
        temp.persist(path).context("cannot replace skill")?;
    } else {
        temp.persist_noclobber(path)
            .context("cannot install skill (destination appeared during install)")?;
    }
    println!("Installed: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_install_is_explicit_idempotent_and_preserves_other_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("skills/zellij-notes-todos/SKILL.md");
        install(&path, true, false).unwrap();
        assert!(!path.parent().unwrap().exists());
        install(&path, false, false).unwrap();
        let unrelated = path.with_file_name("personal.md");
        fs::write(&unrelated, "mine").unwrap();
        install(&path, false, false).unwrap();
        fs::write(&path, "custom instructions").unwrap();
        assert!(install(&path, false, false).is_err());
        assert!(install(&path, true, false).is_err());
        install(&path, true, true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "custom instructions");
        install(&path, false, true).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), SKILL);
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "mine");
    }
}
