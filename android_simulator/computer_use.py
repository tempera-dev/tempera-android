from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import tempfile
import time
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

from . import adb as adb_module
from .config import Toolchain
from .errors import AndroidSimError


_BOUNDS = re.compile(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]")


@dataclass(frozen=True)
class Rect:
    left: int
    top: int
    right: int
    bottom: int

    @property
    def center(self) -> tuple[int, int]:
        return ((self.left + self.right) // 2, (self.top + self.bottom) // 2)

    @property
    def area(self) -> int:
        return max(0, self.right - self.left) * max(0, self.bottom - self.top)


@dataclass(frozen=True)
class UINode:
    ref: str
    text: str
    content_desc: str
    resource_id: str
    class_name: str
    package: str
    bounds: Rect
    clickable: bool
    enabled: bool
    focusable: bool
    scrollable: bool
    selected: bool
    checked: bool
    editable: bool = False
    password: bool = False
    input_focused: bool = False
    long_clickable: bool = False

    @property
    def label(self) -> str:
        return self.text or self.content_desc or self.resource_id.rsplit("/", 1)[-1]

    def compact(self) -> dict[str, Any]:
        value: dict[str, Any] = {
            "ref": self.ref,
            "label": self.label,
            "class": self.class_name.rsplit(".", 1)[-1],
            "bounds": [self.bounds.left, self.bounds.top, self.bounds.right, self.bounds.bottom],
        }
        if self.text and self.text != self.label:
            value["text"] = self.text
        if self.content_desc and self.content_desc != self.label:
            value["desc"] = self.content_desc
        if self.resource_id:
            value["id"] = self.resource_id
        if self.clickable:
            value["clickable"] = True
        if self.long_clickable:
            value["long_clickable"] = True
        if self.editable:
            value["editable"] = True
        if self.scrollable:
            value["scrollable"] = True
        if self.checked:
            value["checked"] = True
        if self.selected:
            value["selected"] = True
        if self.input_focused:
            value["input_focused"] = True
        if self.password:
            value["password"] = True
        if not self.enabled:
            value["enabled"] = False
        return value


@dataclass(frozen=True)
class Observation:
    serial: str
    package: str
    activity: str
    width: int
    height: int
    nodes: tuple[UINode, ...]
    captured_at: float
    latency_ms: float
    revision: int = 0

    def _ranked(self, max_nodes: int) -> list[UINode]:
        return sorted(
            self.nodes,
            key=lambda n: (
                not (n.clickable or n.editable or n.scrollable),
                not bool(n.label),
                -n.bounds.area,
                n.ref,
            ),
        )[:max_nodes]

    def _hash_body(self) -> dict[str, Any]:
        return {
            "package": self.package,
            "activity": self.activity,
            "screen": [self.width, self.height],
            "nodes": [node.compact() for node in self._ranked(180)],
        }

    @property
    def state_hash(self) -> str:
        payload = json.dumps(self._hash_body(), sort_keys=True, separators=(",", ":"))
        return hashlib.blake2s(payload.encode(), digest_size=12).hexdigest()

    def compact(self, *, max_nodes: int = 180) -> dict[str, Any]:
        value = {
            "serial": self.serial,
            "package": self.package,
            "activity": self.activity,
            "screen": [self.width, self.height],
            "state_hash": self.state_hash,
            "latency_ms": round(self.latency_ms, 1),
            "nodes": [node.compact() for node in self._ranked(max_nodes)],
        }
        if self.revision:
            value["revision"] = self.revision
        return value


@dataclass(frozen=True)
class ActionResult:
    action: dict[str, Any]
    ok: bool
    latency_ms: float
    detail: str = ""


class StaleStateError(AndroidSimError):
    def __init__(self, observation: Observation):
        super().__init__("Android UI changed before the planned action could execute")
        self.observation = observation


class DeviceController:
    """Low-overhead Android computer-use primitives over ADB.

    This remains the zero-install fallback. The native accessibility bridge implements the
    same contract and removes UIAutomator/ADB process startup from the hot path.
    """

    transport_name = "adb-uiautomator"

    def __init__(self, toolchain: Toolchain, serial: str):
        self.toolchain = toolchain
        self.serial = serial
        self._last_observation: Observation | None = None

    def _shell(self, args: list[str], *, check: bool = True) -> str:
        return adb_module.shell(self.toolchain, self.serial, args, check=check, quiet=True)

    def _metadata(self) -> tuple[str, str, int, int]:
        raw = self._shell(
            [
                "sh",
                "-c",
                "wm size; echo __ANDROID_SIM_WINDOW__; dumpsys window windows | grep -m 1 -E 'mCurrentFocus|mFocusedApp'",
            ],
            check=False,
        )
        size = re.search(r"(\d+)x(\d+)", raw)
        width, height = (int(size.group(1)), int(size.group(2))) if size else (1080, 1920)
        component = re.search(r"([A-Za-z0-9_.$]+/[A-Za-z0-9_.$]+)", raw)
        if not component:
            return "", "", width, height
        package, _, activity = component.group(1).partition("/")
        return package, activity, width, height

    def _hierarchy_xml(self) -> str:
        xml = self._shell(["uiautomator", "dump", "--compressed", "/dev/tty"], check=False)
        if "<hierarchy" not in xml:
            xml = self._shell(["uiautomator", "dump", "/dev/tty"], check=False)
        if "<hierarchy" not in xml:
            raise AndroidSimError("Could not read Android UI hierarchy")
        return xml[xml.find("<hierarchy") :]

    @staticmethod
    def _parse_nodes(xml: str) -> tuple[UINode, ...]:
        try:
            root = ET.fromstring(xml)
        except ET.ParseError as exc:
            raise AndroidSimError(f"Invalid UI hierarchy XML: {exc}") from exc
        nodes: list[UINode] = []
        for index, element in enumerate(root.iter("node")):
            attrs = element.attrib
            match = _BOUNDS.fullmatch(attrs.get("bounds", ""))
            if not match:
                continue
            left, top, right, bottom = (int(v) for v in match.groups())
            if right <= left or bottom <= top:
                continue
            class_name = attrs.get("class", "").strip()
            password = attrs.get("password") == "true"
            nodes.append(UINode(
                ref=f"n{index}",
                text="" if password else attrs.get("text", "").strip(),
                content_desc=attrs.get("content-desc", "").strip(),
                resource_id=attrs.get("resource-id", "").strip(),
                class_name=class_name,
                package=attrs.get("package", "").strip(),
                bounds=Rect(left, top, right, bottom),
                clickable=attrs.get("clickable") == "true",
                enabled=attrs.get("enabled", "true") == "true",
                focusable=attrs.get("focusable") == "true",
                scrollable=attrs.get("scrollable") == "true",
                selected=attrs.get("selected") == "true",
                checked=attrs.get("checked") == "true",
                editable=attrs.get("editable") == "true" or class_name.endswith("EditText"),
                password=password,
                input_focused=attrs.get("focused") == "true",
                long_clickable=attrs.get("long-clickable") == "true",
            ))
        return tuple(nodes)

    def observe(self) -> Observation:
        started = time.perf_counter()
        with ThreadPoolExecutor(max_workers=2) as executor:
            xml_future = executor.submit(self._hierarchy_xml)
            metadata_future = executor.submit(self._metadata)
            xml = xml_future.result()
            package, activity, width, height = metadata_future.result()
        observation = Observation(
            serial=self.serial,
            package=package,
            activity=activity,
            width=width,
            height=height,
            nodes=self._parse_nodes(xml),
            captured_at=time.time(),
            latency_ms=(time.perf_counter() - started) * 1000,
        )
        self._last_observation = observation
        return observation

    def screenshot(self, destination: Path | None = None) -> Path:
        if destination is None:
            fd, name = tempfile.mkstemp(prefix="android-agent-", suffix=".png")
            os.close(fd)
            destination = Path(name)
        remote = "/sdcard/.android-sim-screen.png"
        self._shell(["screencap", "-p", remote])
        adb_module.adb(self.toolchain, self.serial, ["pull", remote, destination], quiet=True)
        return destination

    def find(self, selector: str, observation: Observation | None = None) -> UINode:
        obs = observation or self._last_observation or self.observe()
        for node in obs.nodes:
            if node.ref == selector:
                return node
        needle = selector.casefold()
        candidates = [node for node in obs.nodes if (
            needle in node.text.casefold()
            or needle in node.content_desc.casefold()
            or needle in node.resource_id.casefold()
            or needle in node.label.casefold()
        )]
        if not candidates:
            raise AndroidSimError(f"No UI node matches {selector!r}")
        candidates.sort(key=lambda n: (not n.clickable, not n.enabled, n.bounds.area))
        return candidates[0]

    @staticmethod
    def _input_text(value: str) -> str:
        return value.replace("%", "%25").replace(" ", "%s")

    def _compile_action(self, action: dict[str, Any], observation: Observation | None) -> tuple[str, str]:
        kind = str(action.get("type", ""))
        detail = ""
        obs = observation or self._last_observation
        try:
            if kind == "tap":
                if "ref" in action or "selector" in action:
                    node = self.find(str(action.get("ref") or action.get("selector")), obs)
                    x, y = node.bounds.center
                    detail = node.label
                else:
                    x, y = int(action["x"]), int(action["y"])
                return f"input tap {x} {y}", detail
            if kind == "long_press":
                node = self.find(str(action.get("ref") or action.get("selector")), obs)
                x, y = node.bounds.center
                duration = max(1, min(int(action.get("duration_ms", 700)), 10000))
                return f"input swipe {x} {y} {x} {y} {duration}", node.label
            if kind == "type":
                value = str(action.get("text", ""))
                encoded = shlex.quote(self._input_text(value))
                prefix = ""
                if action.get("clear"):
                    chars = max(0, min(int(action.get("clear_chars", 120)), 300))
                    prefix = (
                        "input keyevent KEYCODE_MOVE_END; "
                        f"i=0; while [ $i -lt {chars} ]; do input keyevent KEYCODE_DEL; i=$((i+1)); done; "
                    )
                return prefix + f"input text {encoded}", f"{len(value)} chars"
            if kind == "key":
                return f"input keyevent {shlex.quote(str(action['key']))}", detail
            if kind == "back":
                return "input keyevent KEYCODE_BACK", detail
            if kind == "home":
                return "input keyevent KEYCODE_HOME", detail
            if kind == "recents":
                return "input keyevent KEYCODE_APP_SWITCH", detail
            if kind == "notifications":
                return "cmd statusbar expand-notifications", detail
            if kind == "enter":
                return "input keyevent KEYCODE_ENTER", detail
            if kind == "swipe":
                x1, y1 = int(action["x1"]), int(action["y1"])
                x2, y2 = int(action["x2"]), int(action["y2"])
                duration = max(1, min(int(action.get("duration_ms", 220)), 10000))
                return f"input swipe {x1} {y1} {x2} {y2} {duration}", detail
            if kind == "scroll":
                width, height = (obs.width, obs.height) if obs else (1080, 1920)
                direction = str(action.get("direction", "down"))
                amount = max(0.1, min(float(action.get("amount", 0.62)), 0.8))
                x = width // 2
                hi, lo = int(height * 0.78), int(height * max(0.12, 0.78 - amount))
                if direction == "up":
                    x1, y1, x2, y2 = x, lo, x, hi
                elif direction == "left":
                    x1, y1, x2, y2 = int(width * 0.24), height // 2, int(width * 0.78), height // 2
                elif direction == "right":
                    x1, y1, x2, y2 = int(width * 0.78), height // 2, int(width * 0.24), height // 2
                else:
                    x1, y1, x2, y2 = x, hi, x, lo
                return f"input swipe {x1} {y1} {x2} {y2} 180", detail
            if kind == "launch":
                package = shlex.quote(str(action["package"]))
                return f"monkey -p {package} -c android.intent.category.LAUNCHER 1 >/dev/null", detail
            if kind == "wait":
                seconds = max(0.0, min(float(action.get("seconds", 0.5)), 10.0))
                return f"sleep {seconds:.3f}", detail
            raise AndroidSimError(f"Unsupported computer-use action: {kind!r}")
        except (KeyError, TypeError, ValueError) as exc:
            raise AndroidSimError(f"Malformed {kind!r} action: {action}") from exc

    def act(self, action: dict[str, Any], observation: Observation | None = None) -> ActionResult:
        started = time.perf_counter()
        command, detail = self._compile_action(action, observation)
        self._shell(["sh", "-c", "set -e; " + command])
        self._last_observation = None
        return ActionResult(action=action, ok=True, latency_ms=(time.perf_counter() - started) * 1000, detail=detail)

    def macro(self, actions: Iterable[dict[str, Any]], *, max_actions: int = 12) -> list[ActionResult]:
        batch = list(actions)
        if len(batch) > max_actions:
            raise AndroidSimError(f"Macro exceeds {max_actions} action limit")
        if not batch:
            return []
        obs = self._last_observation
        commands: list[str] = []
        details: list[str] = []
        selector_seen = False
        for action in batch:
            uses_selector = bool(action.get("ref") or action.get("selector"))
            if uses_selector and selector_seen:
                raise AndroidSimError("A macro cannot reuse semantic selectors after a UI-changing selector action")
            command, detail = self._compile_action(action, obs)
            commands.append(command)
            details.append(detail)
            selector_seen = selector_seen or uses_selector
        started = time.perf_counter()
        self._shell(["sh", "-c", "set -e; " + "; ".join(commands)])
        elapsed_ms = (time.perf_counter() - started) * 1000
        self._last_observation = None
        per_action = elapsed_ms / len(batch)
        return [
            ActionResult(action=action, ok=True, latency_ms=per_action, detail=detail)
            for action, detail in zip(batch, details)
        ]

    def act_and_observe(
        self,
        actions: Iterable[dict[str, Any]],
        observation: Observation,
        *,
        timeout_ms: int = 900,
    ) -> tuple[list[ActionResult], Observation]:
        self._last_observation = observation
        results = self.macro(actions)
        # UIAutomator has no event channel. A short bounded settle avoids immediately reading a pre-action frame.
        if timeout_ms > 0:
            time.sleep(min(timeout_ms, 120) / 1000.0)
        return results, self.observe()


def action_schema() -> dict[str, Any]:
    return {
        "types": [
            "tap", "long_press", "type", "key", "back", "home", "recents",
            "notifications", "enter", "swipe", "scroll", "launch", "wait"
        ],
        "selector": "Prefer ref from observation. text/resource-id/content-description selectors are also accepted.",
        "batching": "Batch deterministic non-selector follow-ups; only one semantic selector action is allowed per state.",
        "revision": "Native bridge actions are rejected if the observed UI revision changed before execution.",
    }
