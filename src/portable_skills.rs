//! The portable skill is part of the binary; installation is explicit.
use std::{fs, io::Write, path::Path};
use yinz_runtime::errors::prelude::*;

fn install_content(path: &Path, content: &str, dry_run: bool, force: bool) -> Result<()> {
    match fs::read(path) {
        Ok(bytes) if bytes == content.as_bytes() => {
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
    temp.write_all(content.as_bytes())
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

const BUNDLE: &[(&str, &str)] = &[
    (
        "yinz-sessions",
        include_str!("../assets/skills/yinz-sessions/SKILL.md"),
    ),
    (
        "yinz-agents",
        include_str!("../assets/skills/yinz-agents/SKILL.md"),
    ),
    (
        "yinz-plugin-data",
        include_str!("../assets/skills/yinz-plugin-data/SKILL.md"),
    ),
    (
        "yinz-plugins",
        include_str!("../assets/skills/yinz-plugins/SKILL.md"),
    ),
];

pub(crate) fn run(command: yinz_cli::args::Skills) -> Result<()> {
    use crate::agent_hooks::{agent_root, home, selected_agents};
    use yinz_cli::args::Skills;
    match command {
        Skills::List => println!(
            "{}",
            serde_json::json!(BUNDLE.iter().map(|(name, _)| name).collect::<Vec<_>>())
        ),
        Skills::Show { name } => {
            let (_, content) = BUNDLE
                .iter()
                .find(|(id, _)| *id == name)
                .ok_or_else(|| anyhow!("unknown skill {name}; use yinz skills list"))?;
            print!("{content}");
        },
        Skills::Install {
            names,
            agent,
            dest,
            dry_run,
            force,
        } => {
            for name in &names {
                if !BUNDLE.iter().any(|(id, _)| *id == name) {
                    return Err(anyhow!("unknown skill {name}; use yinz skills list"));
                }
            }
            let roots = match dest {
                Some(dest) => vec![dest],
                None => selected_agents(agent, false)?
                    .into_iter()
                    .map(|agent| {
                        agent_root(
                            agent,
                            std::env::var_os("CLAUDE_CONFIG_DIR"),
                            std::env::var_os("CODEX_HOME"),
                            std::env::var_os("PI_CODING_AGENT_DIR"),
                            std::env::var_os("XDG_CONFIG_HOME"),
                            home,
                        )
                        .map(|root| root.join("skills"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            };
            // Preflight every destination, so a conflict does not leave half a bundle installed.
            let selected: Vec<_> = roots
                .iter()
                .flat_map(|root| {
                    BUNDLE
                        .iter()
                        .filter(|(name, _)| names.is_empty() || names.iter().any(|n| n == name))
                        .map(move |(name, content)| (root.join(name).join("SKILL.md"), *content))
                })
                .collect();
            for (path, content) in &selected {
                match fs::read(path) {
                    Ok(bytes) if bytes != content.as_bytes() && !force => {
                        return Err(anyhow!(
                            "{} differs; use --force to replace it",
                            path.display()
                        ))
                    },
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error).context(format!("cannot read {}", path.display()))
                    },
                    _ => {},
                }
            }
            for (path, content) in selected {
                install_content(&path, content, dry_run, force)?;
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_install_is_explicit_idempotent_and_preserves_other_files() {
        const SKILL: &str = include_str!("../assets/skills/yinz-sessions/SKILL.md");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("skills/yinz-sessions/SKILL.md");
        install_content(&path, SKILL, true, false).unwrap();
        assert!(!path.parent().unwrap().exists());
        install_content(&path, SKILL, false, false).unwrap();
        let unrelated = path.with_file_name("personal.md");
        fs::write(&unrelated, "mine").unwrap();
        install_content(&path, SKILL, false, false).unwrap();
        fs::write(&path, "custom instructions").unwrap();
        assert!(install_content(&path, SKILL, false, false).is_err());
        assert!(install_content(&path, SKILL, true, false).is_err());
        install_content(&path, SKILL, true, true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "custom instructions");
        install_content(&path, SKILL, false, true).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), SKILL);
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "mine");
    }
}
