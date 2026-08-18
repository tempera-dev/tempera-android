from __future__ import annotations

import os
import re
from typing import Any

from .errors import AndroidSimError


_ALIAS = re.compile(r"^[A-Za-z][A-Za-z0-9_.-]{0,63}$")


def load_secret_capabilities(specs: list[str]) -> dict[str, str]:
    """Load explicitly authorized NAME=ENV_VAR mappings without putting secret values in argv."""
    result: dict[str, str] = {}
    for spec in specs:
        if not isinstance(spec, str) or "=" not in spec:
            raise AndroidSimError("--secret must use NAME=ENV_VAR")
        alias, env_name = spec.split("=", 1)
        alias = alias.strip()
        env_name = env_name.strip()
        if _ALIAS.fullmatch(alias) is None:
            raise AndroidSimError(f"Invalid secret alias: {alias!r}")
        if _ALIAS.fullmatch(env_name) is None:
            raise AndroidSimError(f"Invalid secret environment variable name: {env_name!r}")
        if alias in result:
            raise AndroidSimError(f"Duplicate secret alias: {alias}")
        value = os.environ.get(env_name)
        if value is None:
            raise AndroidSimError(f"Secret environment variable is not set: {env_name}")
        result[alias] = value
    return result


def resolve_secret_action(action: dict[str, Any], secrets: dict[str, str]) -> dict[str, Any]:
    """Resolve one planner action after planning and before device execution."""
    if "secret_ref" not in action:
        return dict(action)
    if action.get("type") != "type":
        raise AndroidSimError("secret_ref is only valid for type actions")
    if "text" in action:
        raise AndroidSimError("type action cannot provide both text and secret_ref")
    alias = action.get("secret_ref")
    if not isinstance(alias, str) or alias not in secrets:
        raise AndroidSimError(f"Planner requested an unauthorized secret_ref: {alias!r}")
    resolved = dict(action)
    resolved.pop("secret_ref", None)
    resolved["text"] = secrets[alias]
    return resolved


def secret_contract(secrets: dict[str, str]) -> dict[str, Any]:
    return {
        "available_refs": sorted(secrets),
        "usage": "For an authorized credential type action, set secret_ref to one available alias and omit text. The value is resolved locally after planning.",
        "rules": [
            "never invent a secret_ref",
            "never ask for or echo a secret value",
            "secret_ref is valid only on type actions",
        ],
    }
