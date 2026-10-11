//! Generated, workspace-local Mother instructions. Operational memory is separate.
use crate::errors::prelude::*;
use std::{fs, io::Write, path::Path};

pub const SKILL: &str = include_str!("../../assets/skills/yinz-mother/SKILL.md");
const BOOTSTRAP: &str = "# Mother\n\nRead .agents/skills/yinz-mother/SKILL.md, then .agents/skills/yinz-agents/SKILL.md and .agents/skills/yinz-sessions/SKILL.md. Load durable memory with `yinz mother checkpoint` and reconcile `yinz mother inbox` before coordinating this session. Follow the user's scope and permission boundaries.\n";

pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Mother file has no parent")?;
    fs::create_dir_all(parent).context("create Mother file directory")?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("create Mother temporary file")?;
    temp.write_all(bytes).context("write Mother file")?;
    temp.as_file().sync_all().context("sync Mother file")?;
    temp.persist(path).context("replace Mother file")?;
    fs::File::open(parent)
        .context("open Mother directory")?
        .sync_all()
        .context("sync Mother directory")
}

pub fn provision(work: &Path) -> Result<()> {
    for relative in [".agents", ".agents/skills", ".claude", ".claude/skills"] {
        if work.join(relative).is_symlink() {
            bail!("Mother instructions directory must not be a symlink: {relative}");
        }
    }
    for (name, content) in [
        ("yinz-mother", SKILL),
        (
            "yinz-agents",
            include_str!("../../assets/skills/yinz-agents/SKILL.md"),
        ),
        (
            "yinz-sessions",
            include_str!("../../assets/skills/yinz-sessions/SKILL.md"),
        ),
    ] {
        for root in [".agents/skills", ".claude/skills"] {
            if work.join(root).join(name).is_symlink() {
                bail!("Mother skill directory must not be a symlink: {name}");
            }
            replace(
                &work.join(root).join(name).join("SKILL.md"),
                content.as_bytes(),
            )?;
        }
    }
    let executable =
        crate::own_binary::server_binary().context("locate Mother session executable")?;
    let bootstrap = format!("{BOOTSTRAP}\nThe session runs this Yinz executable: {executable:?}. Use this path for commands if `yinz` on PATH is a different version.\n");
    for name in ["AGENTS.md", "CLAUDE.md"] {
        replace(&work.join(name), bootstrap.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/mother.rs"]
mod tests;
