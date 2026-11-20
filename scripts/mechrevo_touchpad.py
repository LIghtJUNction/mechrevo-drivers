#!/usr/bin/python3 -I
# SPDX-License-Identifier: GPL-2.0-or-later
"""Control verified Linux touchpad input interfaces, independently of a desktop.

Installed entry: /usr/lib/mechrevo-control/touchpad-helper. There are no caller
supplied paths or environment-selected backends. Mutations require root; the
packaged polkit action decides which callers may invoke this narrow helper.
"""

import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys


SYS_ROOT = Path("/sys")
STATE_DIR = Path("/var/lib/mechrevo-touchpad")
LOCK_PATH = Path("/run/mechrevo-touchpad.lock")
_TRUST_ROOT = Path("/")
_OWNER_UID = 0
_SEAT = re.compile(r"seat[A-Za-z0-9_-]{1,60}\Z")
_EVENT = re.compile(r"event[0-9]+\Z")
_INPUT = re.compile(r"input[0-9]+\Z")
_OPEN = os.O_CLOEXEC | os.O_NOFOLLOW


class TouchpadError(RuntimeError):
    def __init__(self, message, rollback_errors=()):
        super().__init__(message)
        self.rollback_errors = list(rollback_errors)


def _trusted(info, label, directory=False):
    expected_type = stat.S_ISDIR if directory else stat.S_ISREG
    if not expected_type(info.st_mode) or info.st_uid != _OWNER_UID or info.st_mode & 0o022:
        raise TouchpadError(f"Untrusted ownership, permissions or type: {label}")
    if not directory and info.st_nlink != 1:
        raise TouchpadError(f"Hard-linked privileged file is not allowed: {label}")


@contextlib.contextmanager
def _directory(path, create=False):
    """Walk trusted directories using descriptors, without following symlinks."""
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts:
        raise TouchpadError("Expected an absolute, normalized internal path")
    try:
        parts = path.relative_to(_TRUST_ROOT).parts
    except ValueError as exc:
        raise TouchpadError("Internal path escaped the trusted root") from exc
    fd = os.open(_TRUST_ROOT, os.O_RDONLY | os.O_DIRECTORY | _OPEN)
    try:
        _trusted(os.fstat(fd), _TRUST_ROOT, directory=True)
        for index, part in enumerate(parts):
            try:
                next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | _OPEN, dir_fd=fd)
            except FileNotFoundError:
                if not create or index != len(parts) - 1:
                    raise
                os.mkdir(part, 0o755, dir_fd=fd)
                next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | _OPEN, dir_fd=fd)
            os.close(fd)
            fd = next_fd
            _trusted(os.fstat(fd), path, directory=True)
        yield fd
    finally:
        os.close(fd)


@contextlib.contextmanager
def _file(path, flags=os.O_RDONLY):
    with _directory(Path(path).parent) as directory:
        fd = os.open(Path(path).name, flags | _OPEN, dir_fd=directory)
        try:
            _trusted(os.fstat(fd), path)
            yield fd
        finally:
            os.close(fd)


def _read(path, limit=4096):
    with _file(path) as fd:
        data = os.read(fd, limit + 1)
    if len(data) > limit:
        raise TouchpadError(f"Unexpectedly large value: {path}")
    return data.decode("utf-8", errors="strict")


def _command(argv):
    with _file(Path(argv[0])):
        pass
    result = subprocess.run(
        argv, check=False, text=True, encoding="utf-8", errors="strict",
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15,
        env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"},
    )
    if result.returncode:
        raise TouchpadError(f"{Path(argv[0]).name} failed: {result.stderr.strip()[:500]}")
    if len(result.stdout) > 16 * 1024 * 1024:
        raise TouchpadError("Device/session response exceeds the supported size")
    return result.stdout


def caller_seat():
    """Resolve a pkexec caller from logind, never from caller-supplied seat vars."""
    raw_uid = os.environ.get("PKEXEC_UID")
    if raw_uid is None:
        if os.geteuid() == 0:
            return "seat0"
        raw_uid = str(os.getuid())
    if not re.fullmatch(r"[0-9]{1,10}", raw_uid):
        raise TouchpadError("Invalid PKEXEC_UID")
    uid = str(int(raw_uid))
    sessions = _command([
        "/usr/bin/loginctl", "show-user", uid, "--property=Sessions", "--value", "--no-pager",
    ]).split()
    seats = set()
    for session in sessions:
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", session):
            raise TouchpadError("Invalid logind session identifier")
        output = _command([
            "/usr/bin/loginctl", "show-session", session, "--no-pager",
            "--property=User", "--property=Active", "--property=Remote", "--property=Seat",
        ])
        props = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
        if props.get("User") == uid and props.get("Active") == "yes" and props.get("Remote") == "no":
            seat = props.get("Seat", "")
            # User-manager/background sessions can be active but own no input seat.
            if not seat:
                continue
            if not _SEAT.fullmatch(seat):
                raise TouchpadError("Active caller session has an invalid local seat")
            seats.add(seat)
    if len(seats) != 1:
        raise TouchpadError("Touchpad control requires exactly one active local seat for the caller")
    return seats.pop()


def discover(seat):
    """Use one udev snapshot; validate the actual sysfs input interface afterward."""
    if not _SEAT.fullmatch(seat):
        raise TouchpadError("Invalid seat")
    database = _command(["/usr/bin/udevadm", "info", "--export-db"])
    devices = {}
    for record in database.split("\n\n"):
        props = {}
        devpath = None
        for line in record.splitlines():
            if line.startswith("P: "):
                devpath = line[3:]
            elif line.startswith("E: ") and "=" in line:
                key, value = line[3:].split("=", 1)
                props[key] = value
        if props.get("ID_INPUT_TOUCHPAD") != "1" or props.get("SUBSYSTEM") != "input":
            continue
        if props.get("ID_INPUT_KEYBOARD") == "1" or props.get("ID_INPUT_TOUCHSCREEN") == "1":
            continue
        if props.get("ID_SEAT", "seat0") != seat:
            continue
        if not devpath or not devpath.startswith("/devices/") or ".." in Path(devpath).parts:
            raise TouchpadError("Touchpad has an invalid udev device path")
        event = SYS_ROOT / devpath.lstrip("/")
        if not _EVENT.fullmatch(event.name) or not _INPUT.fullmatch(event.parent.name):
            continue
        # Normal class links are not used here: udev P is the real /devices path.
        with _directory(event) as event_fd:
            event_identity = os.fstat(event_fd)
        input_path = event.parent
        with _directory(input_path) as directory:
            identity = os.fstat(directory)
        inhibited_path = input_path / "inhibited"
        try:
            with _file(inhibited_path) as fd:
                attribute = os.fstat(fd)
        except FileNotFoundError as exc:
            raise TouchpadError(f"Touchpad has no kernel inhibit interface: {input_path}") from exc
        device = {
            "path": input_path, "name": _read(input_path / "name", 1024).strip(), "seat": seat,
            "identity": (identity.st_dev, identity.st_ino),
            "attribute_identity": (attribute.st_dev, attribute.st_ino),
            "event_path": event,
            "event_identity": (event_identity.st_dev, event_identity.st_ino),
        }
        devices[str(input_path)] = device
    if not devices:
        raise TouchpadError(
            f"No verified touchpad input device found on {seat}; if an older tool unbound it, "
            "restore it with that tool's on action or reboot before using this helper"
        )
    return [devices[key] for key in sorted(devices)]


@contextlib.contextmanager
def _attribute(device, flags=os.O_RDONLY):
    with _directory(device["event_path"]) as event_fd:
        event_info = os.fstat(event_fd)
        if (event_info.st_dev, event_info.st_ino) != device["event_identity"]:
            raise TouchpadError("Touchpad event identity changed; rediscover the device")
    with _directory(device["path"]) as directory:
        info = os.fstat(directory)
        if (info.st_dev, info.st_ino) != device["identity"]:
            raise TouchpadError("Touchpad input identity changed; rediscover the device")
        fd = os.open("inhibited", flags | _OPEN, dir_fd=directory)
        try:
            info = os.fstat(fd)
            _trusted(info, device["path"] / "inhibited")
            if (info.st_dev, info.st_ino) != device["attribute_identity"]:
                raise TouchpadError("Touchpad inhibit interface changed; rediscover the device")
            yield fd
        finally:
            os.close(fd)


def read_enabled(device):
    with _attribute(device) as fd:
        value = os.read(fd, 16).strip()
    if value not in (b"0", b"1"):
        raise TouchpadError(f"Invalid kernel inhibit state for {device['name']}")
    return value == b"0"


def write_enabled(device, enabled):
    payload = b"0\n" if enabled else b"1\n"
    with _attribute(device, os.O_WRONLY) as fd:
        if os.write(fd, payload) != len(payload):
            raise TouchpadError(f"Incomplete touchpad write: {device['name']}")
    if read_enabled(device) != enabled:
        raise TouchpadError(f"Touchpad readback disagrees with the request: {device['name']}")


def snapshot(devices, seat):
    readings = [{
        "name": device["name"], "enabled": read_enabled(device),
        "path": str(device["path"]), "seat": device["seat"],
    } for device in devices]
    values = {device["enabled"] for device in readings}
    return {
        "enabled": all(device["enabled"] for device in readings), "mixed": len(values) > 1,
        "devices": readings, "seat": seat, "scope": "kernel-input",
    }


def _state_name(seat):
    if not _SEAT.fullmatch(seat):
        raise TouchpadError("Invalid seat")
    return "touchpad_state" if seat == "seat0" else f"touchpad_state.{seat}"


def load_state(seat):
    try:
        raw = _read(STATE_DIR / _state_name(seat))
    except FileNotFoundError:
        return None, None
    if raw.strip() in {"0", "1"}:
        raise TouchpadError("Legacy I2C touchpad state requires explicit migration before using kernel inhibition")
    try:
        state = json.loads(raw)
    except ValueError as exc:
        raise TouchpadError("Saved touchpad state is not valid JSON") from exc
    if (not isinstance(state, dict) or set(state) != {"version", "seat", "enabled"}
            or type(state["version"]) is not int or state["version"] != 1
            or state["seat"] != seat or type(state["enabled"]) is not bool):
        raise TouchpadError("Saved touchpad state is invalid")
    return state["enabled"], raw


def _save_raw(seat, raw):
    """Atomic replacement in a root-owned directory; caller holds the lock."""
    name = _state_name(seat)
    with _directory(STATE_DIR, create=True) as directory:
        # Validate an existing destination too, rather than silently replacing a link.
        try:
            info = os.stat(name, dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            _trusted(info, STATE_DIR / name)
        if raw is None:
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
            os.fsync(directory)
            return
        temporary = f".{name}.{secrets.token_hex(12)}"
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _OPEN, 0o600, dir_fd=directory)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as output:
                output.write(raw)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, name, src_dir_fd=directory, dst_dir_fd=directory)
            os.fsync(directory)
        finally:
            try:
                os.unlink(temporary, dir_fd=directory)
            except FileNotFoundError:
                pass


@contextlib.contextmanager
def operation_lock():
    with _directory(LOCK_PATH.parent) as directory:
        fd = os.open(LOCK_PATH.name, os.O_RDWR | os.O_CREAT | _OPEN, 0o600, dir_fd=directory)
        try:
            _trusted(os.fstat(fd), LOCK_PATH)
            fcntl.flock(fd, fcntl.LOCK_EX)
            yield
        finally:
            os.close(fd)


def _control_locked(action, seat):
    saved, old_raw = load_state(seat)
    if action == "restore" and saved is None:
        return {"enabled": None, "mixed": False, "devices": [], "seat": seat, "scope": "kernel-input"}
    devices = discover(seat)
    before = snapshot(devices, seat)
    target = saved if action == "restore" else (
        True if action == "on" or action == "toggle" and before["mixed"]
        else not before["enabled"] if action == "toggle" else False
    )
    attempted = []
    persisting = False
    try:
        for device, previous in zip(devices, before["devices"]):
            if previous["enabled"] != target:
                # Include a write that may have changed hardware before returning an error.
                attempted.append((device, previous["enabled"]))
                write_enabled(device, target)
        after = snapshot(devices, seat)
        if any(device["enabled"] != target for device in after["devices"]):
            raise TouchpadError("Touchpad changed during final readback")
        if action != "restore":
            persisting = True
            _save_raw(seat, json.dumps({"version": 1, "seat": seat, "enabled": target}) + "\n")
        return after
    except (OSError, TouchpadError, UnicodeError) as exc:
        failures = []
        for device, enabled in reversed(attempted):
            try:
                write_enabled(device, enabled)
            except (OSError, TouchpadError, UnicodeError) as rollback:
                failures.append(f"{device['name']}: {rollback}")
        if persisting:
            try:
                _save_raw(seat, old_raw)
            except (OSError, TouchpadError, UnicodeError) as rollback:
                failures.append(f"saved state: {rollback}")
        raise TouchpadError(f"Touchpad change failed: {exc}", failures) from exc


def _lifecycle_locked(action):
    """System hooks operate only seats with our own trustworthy saved state."""
    results, errors = [], []
    try:
        with _directory(STATE_DIR) as directory:
            names = sorted(os.listdir(directory))
    except FileNotFoundError:
        return {"results": results, "errors": errors}
    for name in names:
        if name == "touchpad_state":
            seat = "seat0"
        elif name.startswith("touchpad_state."):
            seat = name[len("touchpad_state."):]
            if not _SEAT.fullmatch(seat) or seat == "seat0":
                errors.append({"file": name, "error": "Invalid saved seat filename"})
                continue
        else:
            continue
        try:
            saved, _ = load_state(seat)
            if action == "release-all" and saved is not False:
                results.append({"seat": seat, "skipped": True, "reason": "No saved disabled state"})
                continue
            result = _control_locked("on" if action == "release-all" else "restore", seat)
            # Successful release also saves enabled, so a concurrent later restore
            # cannot immediately disable the device again while uninstalling.
            results.append(result)
        except (OSError, TouchpadError, ValueError, subprocess.SubprocessError) as exc:
            errors.append({"seat": seat, "error": str(exc),
                           "rollback_errors": getattr(exc, "rollback_errors", [])})
    return {"results": results, "errors": errors}


def control(action):
    lifecycle = action in {"restore-all", "release-all"}
    if action not in {"status", "on", "off", "toggle", "restore"} and not lifecycle:
        raise TouchpadError("Expected status, on, off, toggle or restore; system hooks may use restore-all/release-all")
    if action != "status" and os.geteuid() != 0:
        raise TouchpadError("Touchpad changes require the installed authorization helper (root)")
    if lifecycle and "PKEXEC_UID" in os.environ:
        raise TouchpadError("System lifecycle actions require direct root invocation, not pkexec")
    if action == "status":
        seat = caller_seat()
        return snapshot(discover(seat), seat)
    with operation_lock():
        if lifecycle:
            return _lifecycle_locked(action)
        # Resolve after waiting for the lock: a queued caller may have lost its session.
        return _control_locked(action, caller_seat())


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    try:
        if len(args) != 1:
            raise TouchpadError("Expected exactly one action: status, on, off, toggle or restore")
        result = control(args[0])
    except (TouchpadError, OSError, ValueError, subprocess.SubprocessError) as exc:
        print(json.dumps({"ok": False, "error": str(exc), "rollback_errors": getattr(exc, "rollback_errors", [])}, ensure_ascii=False))
        return 1
    success = not result.get("errors")
    print(json.dumps({"ok": success, **result}, ensure_ascii=False))
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
