"""Resolve new-issue worker defaults from the canonical control checkout."""

from .config import config_path, read_config


SUPPORTED_AGENTS = ("claude", "codex", "muse", "pi", "agy")


def _agent(value, source, *, partial=False):
    if not isinstance(value, dict):
        raise ValueError(f"{source} must be an object with kind, model and/or effort")
    unknown = value.keys() - {"kind", "model", "effort"}
    if unknown:
        raise ValueError(f"{source}: unknown fields: {', '.join(sorted(unknown))}")
    if "kind" in value and value["kind"] not in SUPPORTED_AGENTS:
        raise ValueError(f"{source}.kind must be one of {', '.join(SUPPORTED_AGENTS)}")
    if not partial and "kind" not in value:
        raise ValueError(f"{source}.kind is required")
    for key in ("model", "effort"):
        entry = value.get(key)
        if entry is not None and (not isinstance(entry, str) or not entry.strip()):
            raise ValueError(f"{source}.{key} must be a nonempty string or null (provider default)")
    return value


def resolve_agent(control, inputs):
    """Return a resolved selection, or None for the explicit-agent flow."""
    if inputs.keys() - {"agent"}:
        raise ValueError("start accepts only an optional agent object")
    config = read_config(control)
    selection = {}
    if "agent" in config:
        selection = dict(_agent(config["agent"], f"{config_path(control)}: agent"))
    if "agent" in inputs:
        override = _agent(inputs["agent"], "start agent", partial=True)
        if "kind" in override and override["kind"] != selection.get("kind"):
            selection = {}
        selection.update(override)
        _agent(selection, "resolved start agent (set kind in the input or control configuration)")
    return {key: value for key, value in selection.items() if value is not None} or None
