from __future__ import annotations

import json
import sys
from typing import Any

from .computer_use import DeviceController, StaleStateError
from .errors import AndroidSimError


TOOLS = [
    {
        "name": "android_observe",
        "description": "Read the current Android UI as a compact semantic tree. Prefer this before vision.",
        "inputSchema": {
            "type": "object",
            "properties": {"full": {"type": "boolean", "default": False}},
            "additionalProperties": False,
        },
    },
    {
        "name": "android_act",
        "description": "Execute one Android computer-use action and return its verified next state.",
        "inputSchema": {
            "type": "object",
            "required": ["action"],
            "properties": {"action": {"type": "object"}},
            "additionalProperties": False,
        },
    },
    {
        "name": "android_macro",
        "description": "Execute a bounded deterministic Android action batch without an extra observation payload.",
        "inputSchema": {
            "type": "object",
            "required": ["actions"],
            "properties": {
                "actions": {"type": "array", "items": {"type": "object"}, "maxItems": 12}
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "android_act_observe",
        "description": "Execute a bounded action batch only against the expected UI state, wait for Android change events, and return action receipts plus the next semantic state in one call.",
        "inputSchema": {
            "type": "object",
            "required": ["actions"],
            "properties": {
                "actions": {"type": "array", "items": {"type": "object"}, "maxItems": 12},
                "expectedRevision": {"type": "integer", "minimum": 0},
                "expectedStateHash": {"type": "string"},
                "timeoutMs": {"type": "integer", "minimum": 0, "maximum": 5000, "default": 900}
            },
            "additionalProperties": False,
        },
    },
]


def _ok(request_id: Any, result: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def _error(request_id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def _tool_content(value: Any) -> dict[str, Any]:
    return {
        "content": [{"type": "text", "text": json.dumps(value, separators=(",", ":"))}],
        "isError": False,
    }


def _state_mismatch(observation, arguments: dict[str, Any]) -> bool:
    expected_revision = int(arguments.get("expectedRevision") or 0)
    expected_hash = str(arguments.get("expectedStateHash") or "")
    if expected_revision and observation.revision and expected_revision != observation.revision:
        return True
    return bool(expected_hash and expected_hash != observation.state_hash)


def _fused(controller: DeviceController, arguments: dict[str, Any]) -> dict[str, Any]:
    actions = arguments.get("actions")
    if not isinstance(actions, list):
        raise AndroidSimError("android_act_observe requires an actions array")
    observation = controller.observe()
    if _state_mismatch(observation, arguments):
        return {
            "stale": True,
            "transport": controller.transport_name,
            "observation": observation.compact(),
            "results": [],
        }
    try:
        results, next_observation = controller.act_and_observe(
            actions,
            observation,
            timeout_ms=int(arguments.get("timeoutMs") or 900),
        )
    except StaleStateError as exc:
        return {
            "stale": True,
            "transport": controller.transport_name,
            "observation": exc.observation.compact(),
            "results": [],
        }
    return {
        "stale": False,
        "transport": controller.transport_name,
        "results": [result.__dict__ for result in results],
        "observation": next_observation.compact(),
    }


def _handle(controller: DeviceController, request: dict[str, Any]) -> dict[str, Any] | None:
    request_id = request.get("id")
    method = request.get("method")
    params = request.get("params") or {}
    if method == "notifications/initialized":
        return None
    if method == "initialize":
        requested = params.get("protocolVersion") or "2025-06-18"
        return _ok(request_id, {
            "protocolVersion": requested,
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "jadenfix-android-computer-use", "version": "0.3.0"},
        })
    if method == "ping":
        return _ok(request_id, {})
    if method == "tools/list":
        return _ok(request_id, {"tools": TOOLS})
    if method == "tools/call":
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "android_observe":
            obs = controller.observe()
            value = obs.compact(max_nodes=500 if arguments.get("full") else 180)
            value["transport"] = controller.transport_name
            return _ok(request_id, _tool_content(value))
        if name == "android_act":
            action = arguments.get("action")
            if not isinstance(action, dict):
                raise AndroidSimError("android_act requires an action object")
            return _ok(request_id, _tool_content(_fused(controller, {"actions": [action]})))
        if name == "android_macro":
            actions = arguments.get("actions")
            if not isinstance(actions, list):
                raise AndroidSimError("android_macro requires an actions array")
            results = controller.macro(actions)
            return _ok(request_id, _tool_content([result.__dict__ for result in results]))
        if name == "android_act_observe":
            return _ok(request_id, _tool_content(_fused(controller, arguments)))
        return _error(request_id, -32602, f"Unknown tool: {name}")
    return _error(request_id, -32601, f"Method not found: {method}")


def serve(controller: DeviceController) -> int:
    """Dependency-free MCP stdio provider.

    tempera-mcp can front this process for admission, policy, receipts, routing, and transport.
    The Android provider owns only semantic state and execution, keeping platform concerns separated.
    """
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            response = _handle(controller, request)
        except (json.JSONDecodeError, AndroidSimError, TypeError, ValueError) as exc:
            response = _error(None, -32603, str(exc))
        if response is not None:
            sys.stdout.write(json.dumps(response, separators=(",", ":")) + "\n")
            sys.stdout.flush()
    return 0
