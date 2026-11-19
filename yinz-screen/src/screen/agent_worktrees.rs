use super::Screen;
use std::path::PathBuf;
use yinz_runtime::errors::prelude::*;
impl Screen {
    pub(super) fn refresh_agent_worktrees(&mut self) {
        self.agents
            .agent_worktrees
            .refresh_agent_worktrees(&mut self.core);
    }

    pub(super) fn agent_worktree_resolved(
        &mut self,
        cwd: PathBuf,
        generation: u64,
        root: Option<PathBuf>,
    ) -> Result<()> {
        if self
            .agents
            .agent_worktrees
            .resolve_agent_worktree(&mut self.core, cwd, generation, root)
        {
            self.render()?;
        }
        Ok(())
    }
}
