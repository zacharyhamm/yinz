"""Resolve Agenda configuration from the control checkout's Swamp vault."""

import json
import os
from pathlib import Path
import re
import subprocess


VAULT = "default"
KEY = "AGENDA_WORKSPACE"


def local_environment():
    return {key: value for key, value in os.environ.items() if key not in (
        "SWAMP_SERVE_URL", "SWAMP_SERVER_URL", "SWAMP_REPO_DIR", "SWAMP_WORKFLOWS_DIR",
    )}


def agenda_workspace(control, expected=None, run=None):
    """Fail closed on missing, invalid or changed configuration; never use Agenda's default."""
    arguments = ["swamp", "vault", "read-secret", VAULT, KEY, "--force", "--json",
                 "--repo-dir", str(Path(control).resolve(strict=True)), "--no-telemetry"]
    try:
        if run is None:
            result = subprocess.run(arguments, cwd=control, env=local_environment(),
                                    stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            if result.returncode:
                raise ValueError("vault read failed")
            output = result.stdout
        else:
            output = run(arguments)
        workspace = json.loads(output)["value"]
        if not isinstance(workspace, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", workspace):
            raise ValueError("invalid workspace")
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        raise ValueError(
            f"Cannot read a valid Agenda workspace from vault {VAULT}/{KEY}; "
            f"configure it with swamp vault put {VAULT} {KEY} --repo-dir <control-checkout>"
        ) from error
    if expected is not None and workspace != expected:
        raise ValueError("Vault Agenda workspace does not match the requested or recorded issue workspace")
    return workspace
