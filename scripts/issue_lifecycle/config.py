"""Read and validate the control checkout's `.yinz/issue-lifecycle.json`.

This module imports nothing from its siblings, so the launcher can load it as
`issue_lifecycle.config` and the Git helper as top-level `config`.
"""

import json
from pathlib import Path
import re


CONFIG_RELATIVE = (".yinz", "issue-lifecycle.json")
# Matches CheckId in extensions/behaviors/_lib/lifecycle.ts.
CHECK_ID = re.compile(r"[a-z0-9][a-z0-9_-]{0,47}")
BASELINE_LIMIT = 16
# Static checks the merge gate would fail on, run by every verification before
# the plan checks. `cargo x ci check` runs them in this order.
DEFAULT_BASELINE_CHECKS = (
    {"id": "format", "command": ["cargo", "x", "format", "--check"]},
    # The boundary check has no xtask subcommand; the filter selects one test.
    # Branches based before that test existed (ffb0befd0) match nothing, which
    # nextest fails with exit 4 by default; --no-tests=warn passes them, and the
    # merge gate still checks the boundaries on the rebased commit.
    {"id": "dependency-boundaries", "command": [
        "cargo", "x", "test", "-p", "xtask", "--no-tests=warn", "-E",
        "test(=metadata::tests::current_workspace_respects_dependency_boundaries)",
    ]},
    {"id": "lint", "command": ["cargo", "x", "lint"]},
)
TOP_LEVEL_KEYS = {"agent", "verification"}
VERIFICATION_KEYS = {"baselineChecks", "rebaseProbe"}


def config_path(control):
    return Path(control).joinpath(*CONFIG_RELATIVE)


def read_config(control):
    """The parsed configuration, or {} when the file does not exist."""
    path = config_path(control)
    try:
        contents = path.read_text()
    except FileNotFoundError:
        return {}
    except (OSError, UnicodeError) as error:
        raise ValueError(f"Cannot read {path}: {error}") from error
    try:
        config = json.loads(contents)
    except ValueError as error:
        raise ValueError(f"Invalid JSON in {path}: {error}") from error
    if not isinstance(config, dict) or config.keys() - TOP_LEVEL_KEYS:
        raise ValueError(f"{path} must be an object containing only optional 'agent' and 'verification'")
    if "verification" in config:
        validate_verification(config["verification"], f"{path}: verification")
    return config


def _baseline_check(entry, source):
    if not isinstance(entry, dict) or set(entry) != {"id", "command"}:
        raise ValueError(f"{source} must be an object with exactly id and command")
    if not isinstance(entry["id"], str) or not CHECK_ID.fullmatch(entry["id"]):
        raise ValueError(f"{source}.id must match {CHECK_ID.pattern}")
    command = entry["command"]
    if (
        not isinstance(command, list) or not command
        or any(not isinstance(value, str) or "\x00" in value for value in command)
        or not command[0].strip()
    ):
        raise ValueError(f"{source}.command must be a nonempty list of strings without NUL "
                         "whose executable is not blank")
    return entry


def validate_verification(value, source):
    if not isinstance(value, dict):
        raise ValueError(f"{source} must be an object with optional baselineChecks and rebaseProbe")
    unknown = value.keys() - VERIFICATION_KEYS
    if unknown:
        raise ValueError(f"{source}: unknown fields: {', '.join(sorted(unknown))}")
    if "baselineChecks" in value:
        checks = value["baselineChecks"]
        if not isinstance(checks, list) or len(checks) > BASELINE_LIMIT:
            raise ValueError(f"{source}.baselineChecks must be a list of at most {BASELINE_LIMIT} checks")
        for index, entry in enumerate(checks):
            _baseline_check(entry, f"{source}.baselineChecks[{index}]")
        ids = [entry["id"] for entry in checks]
        if len(set(ids)) != len(ids):
            raise ValueError(f"{source}.baselineChecks IDs must be unique")
    if "rebaseProbe" in value and type(value["rebaseProbe"]) is not bool:
        raise ValueError(f"{source}.rebaseProbe must be a boolean")
    return value


def verification_settings(control):
    """Baseline checks and the probe switch; missing keys take the defaults.

    An empty baselineChecks list is an explicit opt-out.
    """
    section = read_config(control).get("verification", {})
    checks = section.get("baselineChecks")
    return {
        "baselineChecks": [
            {"id": entry["id"], "command": list(entry["command"])}
            for entry in (DEFAULT_BASELINE_CHECKS if checks is None else checks)
        ],
        "rebaseProbe": section.get("rebaseProbe", True),
    }
