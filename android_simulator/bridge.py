from __future__ import annotations

import base64
import json
import os
import re
import secrets
import socket
import threading
import time
from pathlib import Path
from typing import Any, Iterable

from . import adb as adb_module
from .computer_use import (
    ActionResult,
    DeviceController,
    Observation,
    Rect,
    StaleStateError,
    UINode,
)
from .config import Toolchain
from .errors import AndroidSimError
from .util import atomic_write, run


PACKAGE = "dev.jadenfix.androidbridge"
SERVICE = f"{PACKAGE}/.BridgeAccessibilityService"
DEVICE_PORT = 6210
PROTOCOL_VERSION = 2
TOKEN_DIR = Path.home() / ".android-sim" / "bridge"


def _safe_serial(serial: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", serial)


def token_path(serial: str) -> Path:
    return TOKEN_DIR / f"{_safe_serial(serial)}.token"


def _host_token(serial: str, *, create: bool) -> str:
    path = token_path(serial)
    if path.is_file():
        value = path.read_text(encoding="utf-8").strip()
        if len(value) >= 32:
            return value
    if not create:
        raise AndroidSimError("Android bridge token is not configured; run 'android-agent bridge setup'")
    value = secrets.token_hex(32)
    atomic_write(path, value + "\n", mode=0o600)
    return value


def _write_device_token(toolchain: Toolchain, serial: str, token: str) -> None:
    command = f"mkdir -p files; umask 077; printf %s {token} > files/bridge.token"
    adb_module.shell(
        toolchain,
        serial,
        ["run-as", PACKAGE, "sh", "-c", command],
        quiet=True,
    )


def _enabled_services(toolchain: Toolchain, serial: str) -> list[str]:
    raw = adb_module.shell(
        toolchain,
        serial,
        ["settings", "get", "secure", "enabled_accessibility_services"],
        check=False,
        quiet=True,
    ).strip()
    if not raw or raw == "null":
        return []
    return [item for item in raw.split(":") if item]


def bridge_installed(toolchain: Toolchain, serial: str) -> bool:
    result = adb_module.adb(
        toolchain,
        serial,
        ["shell", "pm", "path", PACKAGE],
        check=False,
        quiet=True,
    )
    return result.returncode == 0 and "package:" in result.stdout


def bridge_enabled(toolchain: Toolchain, serial: str) -> bool:
    return SERVICE in _enabled_services(toolchain, serial)


def enable_bridge(toolchain: Toolchain, serial: str) -> None:
    services = _enabled_services(toolchain, serial)
    if SERVICE not in services:
        services.append(SERVICE)
    joined = ":".join(services)
    try:
        adb_module.shell(
            toolchain,
            serial,
            ["settings", "put", "secure", "enabled_accessibility_services", joined],
            quiet=True,
        )
        adb_module.shell(
            toolchain,
            serial,
            ["settings", "put", "secure", "accessibility_enabled", "1"],
            quiet=True,
        )
    except AndroidSimError as exc:
        adb_module.shell(
            toolchain,
            serial,
            ["am", "start", "-a", "android.settings.ACCESSIBILITY_SETTINGS"],
            check=False,
            quiet=True,
        )
        raise AndroidSimError(
            "Could not enable the bridge through emulator secure settings. "
            "Android Accessibility settings were opened; enable 'Android Agent Bridge' once, then retry."
        ) from exc


def disable_bridge(toolchain: Toolchain, serial: str) -> None:
    services = [item for item in _enabled_services(toolchain, serial) if item != SERVICE]
    adb_module.shell(
        toolchain,
        serial,
        ["settings", "put", "secure", "enabled_accessibility_services", ":".join(services)],
        check=False,
        quiet=True,
    )
    if not services:
        adb_module.shell(
            toolchain,
            serial,
            ["settings", "put", "secure", "accessibility_enabled", "0"],
            check=False,
            quiet=True,
        )


def _source_root() -> Path:
    return Path(__file__).resolve().parents[1]


def build_bridge(apk: Path | None = None) -> Path:
    if apk is not None:
        resolved = apk.expanduser().resolve()
        if not resolved.is_file():
            raise AndroidSimError(f"Bridge APK not found: {resolved}")
        return resolved
    root = _source_root()
    script = root / "scripts" / "build-companion.sh"
    if not script.is_file():
        raise AndroidSimError(
            "Bridge sources are not present in this installation. Pass --apk with a prebuilt bridge APK."
        )
    result = run([script], quiet=False)
    lines = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    if not lines:
        raise AndroidSimError("Bridge build did not report an APK path")
    output = Path(lines[-1]).expanduser().resolve()
    if not output.is_file():
        raise AndroidSimError(f"Bridge build output missing: {output}")
    return output


def install_bridge(toolchain: Toolchain, serial: str, apk: Path) -> None:
    adb_module.adb(toolchain, serial, ["install", "-r", "-g", apk], capture=False)


def setup_bridge(toolchain: Toolchain, serial: str, *, apk: Path | None = None) -> dict[str, Any]:
    output = build_bridge(apk)
    install_bridge(toolchain, serial, output)
    token = _host_token(serial, create=True)
    _write_device_token(toolchain, serial, token)
    enable_bridge(toolchain, serial)
    client = BridgeClient(toolchain, serial, token=token, connect_timeout=6.0)
    try:
        health = client.health()
    finally:
        client.close()
    return {
        "installed": True,
        "enabled": True,
        "apk": str(output),
        "service": SERVICE,
        "protocol": health.get("protocol"),
        "server_epoch": health.get("server_epoch"),
        "revision": health.get("revision"),
        "capabilities": health.get("capabilities", []),
    }


def bridge_status(toolchain: Toolchain, serial: str) -> dict[str, Any]:
    installed = bridge_installed(toolchain, serial)
    enabled = bridge_enabled(toolchain, serial) if installed else False
    status: dict[str, Any] = {
        "installed": installed,
        "enabled": enabled,
        "service": SERVICE,
        "reachable": False,
    }
    if installed and enabled and token_path(serial).is_file():
        try:
            client = BridgeClient(toolchain, serial, token=_host_token(serial, create=False), connect_timeout=1.0)
            try:
                status.update(client.health())
                status["reachable"] = True
            finally:
                client.close()
        except AndroidSimError as exc:
            status["error"] = str(exc)
    return status


class BridgeClient:
    """Persistent bridge client with retry-safe request identity.

    Action requests may be retried after a transport failure because protocol v2 carries a
    per-client request ID and server epoch; the bridge caches completed responses and refuses
    requests from a different service epoch instead of risking duplicate side effects.
    """

    def __init__(
        self,
        toolchain: Toolchain,
        serial: str,
        *,
        token: str | None = None,
        connect_timeout: float = 2.0,
    ):
        self.toolchain = toolchain
        self.serial = serial
        self.token = token or _host_token(serial, create=False)
        self.connect_timeout = connect_timeout
        self.host_port = self._allocate_port()
        self.client_id = secrets.token_hex(16)
        self.server_epoch: str | None = None
        self._socket: socket.socket | None = None
        self._reader = None
        self._writer = None
        self._lock = threading.RLock()
        self._request_id = 0
        self._configure_forward()
        self._connect()

    @staticmethod
    def _allocate_port() -> int:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind(("127.0.0.1", 0))
            return int(probe.getsockname()[1])

    def _configure_forward(self) -> None:
        adb_module.adb(
            self.toolchain,
            self.serial,
            ["forward", f"tcp:{self.host_port}", f"tcp:{DEVICE_PORT}"],
            quiet=True,
        )

    def _connect(self) -> None:
        deadline = time.monotonic() + self.connect_timeout
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                sock = socket.create_connection(("127.0.0.1", self.host_port), timeout=0.75)
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                # wait_observe may legally wait 15s plus a bounded quiescence window.
                sock.settimeout(25.0)
                self._socket = sock
                self._reader = sock.makefile("r", encoding="utf-8", newline="\n")
                self._writer = sock.makefile("w", encoding="utf-8", newline="\n")
                return
            except OSError as exc:
                last = exc
                time.sleep(0.1)
        raise AndroidSimError(f"Android native bridge did not become reachable: {last}")

    def _drop_socket(self) -> None:
        for stream in (self._reader, self._writer):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass
        if self._socket is not None:
            try:
                self._socket.close()
            except OSError:
                pass
        self._reader = None
        self._writer = None
        self._socket = None

    def close(self) -> None:
        self._drop_socket()
        adb_module.adb(
            self.toolchain,
            self.serial,
            ["forward", "--remove", f"tcp:{self.host_port}"],
            check=False,
            quiet=True,
        )

    def _ensure_connection(self) -> None:
        if self._reader is not None and self._writer is not None:
            return
        try:
            self._connect()
        except AndroidSimError:
            # Recreate the ADB forwarding rule only when the existing mapping no longer works.
            self._configure_forward()
            self._connect()

    def _wire(self, request: dict[str, Any]) -> dict[str, Any]:
        self._ensure_connection()
        assert self._reader is not None and self._writer is not None
        self._writer.write(json.dumps(request, separators=(",", ":")) + "\n")
        self._writer.flush()
        line = self._reader.readline()
        if not line:
            raise OSError("Android bridge closed the connection")
        try:
            response = json.loads(line)
        except json.JSONDecodeError as exc:
            raise OSError("Android bridge returned invalid JSON") from exc
        if response.get("id") not in (None, request["id"]):
            raise OSError("Android bridge response ID mismatch")
        return response

    def _request(self, op: str, **payload: Any) -> dict[str, Any]:
        with self._lock:
            if op != "health" and not self.server_epoch:
                self.health()
            self._request_id += 1
            request_id = self._request_id
            request: dict[str, Any] = {
                "id": request_id,
                "client_id": self.client_id,
                "token": self.token,
                "op": op,
                **payload,
            }
            if op != "health":
                request["server_epoch"] = self.server_epoch

            response: dict[str, Any] | None = None
            last_transport_error: Exception | None = None
            for attempt in range(2):
                try:
                    response = self._wire(request)
                    break
                except (OSError, ValueError) as exc:
                    last_transport_error = exc
                    self._drop_socket()
                    if attempt == 0:
                        continue
            if response is None:
                raise AndroidSimError(f"Android bridge I/O failed after retry: {last_transport_error}")

            if not response.get("ok"):
                message = str(response.get("error", "unknown error"))
                if "server epoch mismatch" in message:
                    raise AndroidSimError(
                        "Android bridge restarted during a request; action replay was refused to preserve at-most-once execution. "
                        "Refresh observation before continuing."
                    )
                raise AndroidSimError(f"Android bridge error: {message}")
            result = response.get("result")
            if not isinstance(result, dict):
                raise AndroidSimError("Android bridge response missing result object")
            if op == "health":
                epoch = result.get("server_epoch")
                if not isinstance(epoch, str) or not epoch:
                    raise AndroidSimError("Android bridge health response missing server epoch")
                self.server_epoch = epoch
            return result

    def health(self) -> dict[str, Any]:
        return self._request("health")

    def observe(self) -> dict[str, Any]:
        return self._request("observe")

    def act(self, actions: list[dict[str, Any]], *, expected_revision: int = 0) -> dict[str, Any]:
        return self._request("act", actions=actions, expected_revision=expected_revision)

    def act_observe(
        self,
        actions: list[dict[str, Any]],
        *,
        expected_revision: int = 0,
        timeout_ms: int = 900,
        quiet_ms: int = 120,
        max_settle_ms: int = 900,
    ) -> dict[str, Any]:
        return self._request(
            "act_observe",
            actions=actions,
            expected_revision=expected_revision,
            timeout_ms=timeout_ms,
            quiet_ms=quiet_ms,
            max_settle_ms=max_settle_ms,
        )

    def wait_observe(
        self,
        *,
        after_revision: int,
        timeout_ms: int = 2000,
        quiet_ms: int = 120,
        max_settle_ms: int = 900,
    ) -> dict[str, Any]:
        return self._request(
            "wait_observe",
            after_revision=after_revision,
            timeout_ms=timeout_ms,
            quiet_ms=quiet_ms,
            max_settle_ms=max_settle_ms,
        )

    def screenshot(self) -> bytes:
        result = self._request("screenshot")
        try:
            return base64.b64decode(result["png_base64"], validate=True)
        except (KeyError, ValueError) as exc:
            raise AndroidSimError("Android bridge returned an invalid screenshot payload") from exc


class BridgeController(DeviceController):
    transport_name = "accessibility-bridge"

    def __init__(self, toolchain: Toolchain, serial: str, client: BridgeClient):
        super().__init__(toolchain, serial)
        self.client = client
        self.last_transition: dict[str, Any] = {}

    def close(self) -> None:
        self.client.close()

    def _observation(self, payload: dict[str, Any], latency_ms: float) -> Observation:
        screen = payload.get("screen") or [1080, 1920]
        nodes: list[UINode] = []
        for raw in payload.get("nodes", []):
            if not isinstance(raw, dict):
                continue
            bounds = raw.get("bounds") or [0, 0, 0, 0]
            if not isinstance(bounds, list) or len(bounds) != 4:
                continue
            label = str(raw.get("label", ""))
            text = str(raw.get("text", ""))
            desc = str(raw.get("desc", ""))
            resource_id = str(raw.get("id", ""))
            if not text and not desc and not resource_id and label:
                desc = label
            nodes.append(UINode(
                ref=str(raw.get("ref", "")),
                text=text,
                content_desc=desc,
                resource_id=resource_id,
                class_name=str(raw.get("class", "")),
                package=str(raw.get("package", payload.get("package", ""))),
                bounds=Rect(*(int(value) for value in bounds)),
                clickable=bool(raw.get("clickable", False)),
                enabled=bool(raw.get("enabled", True)),
                focusable=bool(raw.get("editable", False) or raw.get("input_focused", False)),
                scrollable=bool(raw.get("scrollable", False)),
                selected=bool(raw.get("selected", False)),
                checked=bool(raw.get("checked", False)),
                editable=bool(raw.get("editable", False)),
                password=bool(raw.get("password", False)),
                input_focused=bool(raw.get("input_focused", False)),
                long_clickable=bool(raw.get("long_clickable", False)),
            ))
        observation = Observation(
            serial=self.serial,
            package=str(payload.get("package", "")),
            activity=str(payload.get("activity", "")),
            width=int(screen[0]),
            height=int(screen[1]),
            nodes=tuple(nodes),
            captured_at=time.time(),
            latency_ms=latency_ms,
            revision=int(payload.get("revision", 0)),
        )
        self._last_observation = observation
        return observation

    def observe(self) -> Observation:
        started = time.perf_counter()
        payload = self.client.observe()
        return self._observation(payload, (time.perf_counter() - started) * 1000)

    def screenshot(self, destination: Path | None = None) -> Path:
        if destination is None:
            import tempfile
            fd, name = tempfile.mkstemp(prefix="android-agent-bridge-", suffix=".png")
            os.close(fd)
            destination = Path(name)
        destination.write_bytes(self.client.screenshot())
        return destination

    def _native_action(self, action: dict[str, Any], observation: Observation) -> dict[str, Any]:
        normalized = dict(action)
        selector = normalized.get("selector")
        if selector and not normalized.get("ref"):
            normalized["ref"] = self.find(str(selector), observation).ref
            normalized.pop("selector", None)
        return normalized

    @staticmethod
    def _result(raw: dict[str, Any], fallback_action: dict[str, Any]) -> ActionResult:
        action = raw.get("action") if isinstance(raw.get("action"), dict) else fallback_action
        return ActionResult(
            action=action,
            ok=bool(raw.get("ok", True)),
            latency_ms=float(raw.get("latency_ms", 0.0)),
            detail=str(raw.get("detail") or raw.get("error") or ""),
        )

    def act(self, action: dict[str, Any], observation: Observation | None = None) -> ActionResult:
        obs = observation or self._last_observation or self.observe()
        if action.get("type") == "key":
            return DeviceController.act(self, action, obs)
        normalized = self._native_action(action, obs)
        payload = self.client.act([normalized], expected_revision=obs.revision)
        if payload.get("stale"):
            fresh = self._observation(payload["observation"], 0.0)
            raise StaleStateError(fresh)
        rows = payload.get("results") or []
        if not rows:
            raise AndroidSimError("Android bridge did not return an action result")
        self._last_observation = None
        return self._result(rows[0], action)

    def macro(self, actions: Iterable[dict[str, Any]], *, max_actions: int = 12) -> list[ActionResult]:
        batch = list(actions)
        if len(batch) > max_actions:
            raise AndroidSimError(f"Macro exceeds {max_actions} action limit")
        if not batch:
            return []
        obs = self._last_observation or self.observe()
        if any(action.get("type") == "key" for action in batch):
            return DeviceController.macro(self, batch, max_actions=max_actions)
        normalized = [self._native_action(action, obs) for action in batch]
        payload = self.client.act(normalized, expected_revision=obs.revision)
        if payload.get("stale"):
            fresh = self._observation(payload["observation"], 0.0)
            raise StaleStateError(fresh)
        rows = payload.get("results") or []
        self._last_observation = None
        return [self._result(row, action) for row, action in zip(rows, batch)]

    def act_and_observe(
        self,
        actions: Iterable[dict[str, Any]],
        observation: Observation,
        *,
        timeout_ms: int = 900,
    ) -> tuple[list[ActionResult], Observation]:
        batch = list(actions)
        if not batch:
            self.last_transition = {
                "changed": False,
                "settled": True,
                "events": 0,
                "revision": observation.revision,
            }
            return [], observation

        if any(action.get("type") == "key" for action in batch):
            self._last_observation = observation
            started = time.perf_counter()
            results = DeviceController.macro(self, batch, max_actions=12)
            payload = self.client.wait_observe(after_revision=observation.revision, timeout_ms=timeout_ms)
            elapsed = (time.perf_counter() - started) * 1000
            self.last_transition = dict(payload.get("transition") or {})
            return results, self._observation(payload["observation"], elapsed)

        normalized = [self._native_action(action, observation) for action in batch]
        started = time.perf_counter()
        payload = self.client.act_observe(
            normalized,
            expected_revision=observation.revision,
            timeout_ms=timeout_ms,
        )
        latency_ms = (time.perf_counter() - started) * 1000
        if payload.get("stale"):
            fresh = self._observation(payload["observation"], latency_ms)
            raise StaleStateError(fresh)
        rows = payload.get("results") or []
        results = [self._result(row, action) for row, action in zip(rows, batch)]
        self.last_transition = dict(payload.get("transition") or {
            "changed": bool(payload.get("changed", False)),
            "settled": bool(payload.get("settled", True)),
        })
        next_observation = self._observation(payload["observation"], latency_ms)
        return results, next_observation


def make_controller(toolchain: Toolchain, serial: str, transport: str = "auto") -> DeviceController:
    if transport not in {"auto", "bridge", "adb"}:
        raise AndroidSimError(f"Unknown Android agent transport: {transport}")
    if transport == "adb":
        return DeviceController(toolchain, serial)
    try:
        client = BridgeClient(toolchain, serial, connect_timeout=0.9 if transport == "auto" else 3.0)
        health = client.health()
        if int(health.get("protocol", 0)) != PROTOCOL_VERSION:
            client.close()
            raise AndroidSimError(f"Unsupported Android bridge protocol: {health.get('protocol')}")
        return BridgeController(toolchain, serial, client)
    except AndroidSimError:
        if transport == "bridge":
            raise
        return DeviceController(toolchain, serial)
