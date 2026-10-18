#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""MECHREVO Gaming Center (机械革命电竞控制台) for Linux.

This tool ports and integrates the complete feature set of the OEM Mechrevo
Gaming Center to Linux on the GM6PX0X (and compatible) platforms:
1. Operating Modes (Office 45W / Gaming 75W / Turbo 130W / Custom)
2. One-Click Fan Boost (一键强冷 / 100% Full Speed) & Auto Fan Control
3. Win Key Lock (电竞游戏防误触锁) via hardware scancode mapping
4. Touchpad Hardware Lock via I2C device bind/unbind
5. Fn Lock (F1-F12 primary vs multimedia hotkeys)
6. 3-Stage Battery Health Care (100% High Capacity / 80% Balanced / 60% Long-Life)
7. Single-Zone RGB Keyboard Lighting & 4-Level Brightness Control
8. Real-time Telemetry Dashboard (Dual Fan RPM, CPU/GPU EC Temperatures, Power)
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import time

PROFILES_PATH = Path("/etc/tcc/profiles")
SETTINGS_PATH = Path("/etc/tcc/settings")
def get_state_dir() -> Path:
    sys_dir = Path("/var/lib/mechrevo-control")
    if sys_dir.exists() and os.access(sys_dir, os.W_OK):
        return sys_dir
    if os.geteuid() == 0:
        try:
            sys_dir.mkdir(parents=True, exist_ok=True)
            return sys_dir
        except Exception:
            pass
    user_dir = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "mechrevo-control"
    try:
        user_dir.mkdir(parents=True, exist_ok=True)
        return user_dir
    except Exception:
        fallback = Path("/tmp/mechrevo-control")
        fallback.mkdir(parents=True, exist_ok=True)
        return fallback


def get_state_file(name: str) -> Path:
    sys_file = Path("/var/lib/mechrevo-control") / name
    if sys_file.exists():
        return sys_file
    return get_state_dir() / name

FN_LOCK_PATH = Path("/sys/devices/platform/tuxedo_keyboard/fn_lock")
BATTERY_PROFILE_PATH = Path("/sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profile")
RGB_LEDS_DIR = Path("/sys/class/leds/rgb:kbd_backlight")
TOUCHPAD_I2C_DEVICE = "i2c-UNIW0001:00"
TOUCHPAD_BIND_PATH = Path("/sys/bus/i2c/drivers/i2c_hid_acpi/bind")
TOUCHPAD_UNBIND_PATH = Path("/sys/bus/i2c/drivers/i2c_hid_acpi/unbind")
TOUCHPAD_BOUND_CHECK = Path(f"/sys/bus/i2c/drivers/i2c_hid_acpi/{TOUCHPAD_I2C_DEVICE}")

# OEM Mechrevo profiles matching original specifications
MECHREVO_PROFILES = [
    {
        "id": "mechrevo_office",
        "name": "机械革命-办公节能模式 (Office)",
        "description": "机械革命原厂办公模式：PL1 45W，低噪节能，静音温控曲线",
        "display": {
            "brightness": 70,
            "useBrightness": True,
            "refreshRate": -1,
            "useRefRate": False,
            "xResolution": -1,
            "yResolution": -1,
            "useResolution": False,
        },
        "cpu": {
            "useMaxPerfGov": False,
            "governor": "powersave",
            "energyPerformancePreference": "power",
            "noTurbo": False,
            "onlineCores": 24,
            "scalingMinFrequency": 800000,
            "scalingMaxFrequency": 3200000,
        },
        "webcam": {"status": True, "useStatus": True},
        "fan": {
            "useControl": True,
            "fanProfile": "Quiet",
            "minimumFanspeed": 0,
            "maximumFanspeed": 100,
            "offsetFanspeed": 0,
            "customFanCurve": {
                "tableCPU": [
                    {"temp": 20, "speed": 10},
                    {"temp": 30, "speed": 12},
                    {"temp": 40, "speed": 15},
                    {"temp": 50, "speed": 22},
                    {"temp": 60, "speed": 35},
                    {"temp": 70, "speed": 50},
                    {"temp": 80, "speed": 70},
                    {"temp": 90, "speed": 85},
                    {"temp": 100, "speed": 90},
                ],
                "tableGPU": [
                    {"temp": 20, "speed": 10},
                    {"temp": 30, "speed": 12},
                    {"temp": 40, "speed": 15},
                    {"temp": 50, "speed": 22},
                    {"temp": 60, "speed": 35},
                    {"temp": 70, "speed": 50},
                    {"temp": 80, "speed": 70},
                    {"temp": 90, "speed": 85},
                    {"temp": 100, "speed": 90},
                ],
            },
        },
        "odmProfile": {"name": ""},
        "odmPowerLimits": {"tdpValues": []},
        "nvidiaPowerCTRLProfile": {"cTGPOffset": 0},
    },
    {
        "id": "mechrevo_gaming",
        "name": "机械革命-游戏平衡模式 (Gaming)",
        "description": "机械革命原厂游戏模式：PL1 75W，动态睿频，平衡温控曲线",
        "display": {
            "brightness": 80,
            "useBrightness": True,
            "refreshRate": -1,
            "useRefRate": False,
            "xResolution": -1,
            "yResolution": -1,
            "useResolution": False,
        },
        "cpu": {
            "useMaxPerfGov": False,
            "governor": "powersave",
            "energyPerformancePreference": "balance_performance",
            "noTurbo": False,
            "onlineCores": 24,
            "scalingMinFrequency": 800000,
            "scalingMaxFrequency": 4900000,
        },
        "webcam": {"status": True, "useStatus": True},
        "fan": {
            "useControl": True,
            "fanProfile": "Balanced",
            "minimumFanspeed": 0,
            "maximumFanspeed": 100,
            "offsetFanspeed": 0,
            "customFanCurve": {
                "tableCPU": [
                    {"temp": 20, "speed": 15},
                    {"temp": 30, "speed": 18},
                    {"temp": 40, "speed": 25},
                    {"temp": 50, "speed": 38},
                    {"temp": 60, "speed": 50},
                    {"temp": 70, "speed": 65},
                    {"temp": 80, "speed": 82},
                    {"temp": 90, "speed": 92},
                    {"temp": 100, "speed": 95},
                ],
                "tableGPU": [
                    {"temp": 20, "speed": 15},
                    {"temp": 30, "speed": 18},
                    {"temp": 40, "speed": 25},
                    {"temp": 50, "speed": 38},
                    {"temp": 60, "speed": 50},
                    {"temp": 70, "speed": 65},
                    {"temp": 80, "speed": 82},
                    {"temp": 90, "speed": 92},
                    {"temp": 100, "speed": 95},
                ],
            },
        },
        "odmProfile": {"name": ""},
        "odmPowerLimits": {"tdpValues": []},
        "nvidiaPowerCTRLProfile": {"cTGPOffset": 25},
    },
    {
        "id": "mechrevo_turbo",
        "name": "机械革命-极速狂暴模式 (Turbo)",
        "description": "机械革命原厂狂暴模式：PL1 130W 满血释放，全核激进，强冷曲线",
        "display": {
            "brightness": 90,
            "useBrightness": True,
            "refreshRate": -1,
            "useRefRate": False,
            "xResolution": -1,
            "yResolution": -1,
            "useResolution": False,
        },
        "cpu": {
            "useMaxPerfGov": True,
            "governor": "performance",
            "energyPerformancePreference": "performance",
            "noTurbo": False,
            "onlineCores": 24,
            "scalingMinFrequency": 800000,
            "scalingMaxFrequency": 5000000,
        },
        "webcam": {"status": True, "useStatus": True},
        "fan": {
            "useControl": True,
            "fanProfile": "Freezy",
            "minimumFanspeed": 0,
            "maximumFanspeed": 100,
            "offsetFanspeed": 0,
            "customFanCurve": {
                "tableCPU": [
                    {"temp": 20, "speed": 22},
                    {"temp": 30, "speed": 28},
                    {"temp": 40, "speed": 40},
                    {"temp": 50, "speed": 55},
                    {"temp": 60, "speed": 72},
                    {"temp": 70, "speed": 85},
                    {"temp": 80, "speed": 95},
                    {"temp": 90, "speed": 98},
                    {"temp": 100, "speed": 100},
                ],
                "tableGPU": [
                    {"temp": 20, "speed": 22},
                    {"temp": 30, "speed": 28},
                    {"temp": 40, "speed": 40},
                    {"temp": 50, "speed": 55},
                    {"temp": 60, "speed": 72},
                    {"temp": 70, "speed": 85},
                    {"temp": 80, "speed": 95},
                    {"temp": 90, "speed": 98},
                    {"temp": 100, "speed": 100},
                ],
            },
        },
        "odmProfile": {"name": ""},
        "odmPowerLimits": {"tdpValues": []},
        "nvidiaPowerCTRLProfile": {"cTGPOffset": 25},
    },
    {
        "id": "mechrevo_fan_boost",
        "name": "机械革命-一键强冷 (Fan Boost)",
        "description": "机械革命原厂一键强冷：双风扇 100% 满转速，极速压制积热",
        "display": {
            "brightness": 80,
            "useBrightness": False,
            "refreshRate": -1,
            "useRefRate": False,
            "xResolution": -1,
            "yResolution": -1,
            "useResolution": False,
        },
        "cpu": {
            "useMaxPerfGov": False,
            "governor": "powersave",
            "energyPerformancePreference": "balance_performance",
            "noTurbo": False,
            "onlineCores": 24,
            "scalingMinFrequency": 800000,
            "scalingMaxFrequency": 4900000,
        },
        "webcam": {"status": True, "useStatus": False},
        "fan": {
            "useControl": True,
            "fanProfile": "Freezy",
            "minimumFanspeed": 100,
            "maximumFanspeed": 100,
            "offsetFanspeed": 0,
            "customFanCurve": {
                "tableCPU": [{"temp": t, "speed": 100} for t in range(20, 101, 10)],
                "tableGPU": [{"temp": t, "speed": 100} for t in range(20, 101, 10)],
            },
        },
        "odmProfile": {"name": ""},
        "odmPowerLimits": {"tdpValues": []},
        "nvidiaPowerCTRLProfile": {"cTGPOffset": 25},
    },
]


def run_cmd(cmd: list[str], check: bool = True) -> str:
    """Run a shell command and return trimmed output."""
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if check and res.returncode != 0:
        raise RuntimeError(f"Command {' '.join(cmd)} failed (code {res.returncode}): {res.stderr.strip()}")
    return res.stdout.strip()


def ensure_state_dir() -> None:
    get_state_dir()


def call_tccd_dbus(method: str, signature: str = "", *args) -> str:
    """Call a method on com.tuxedocomputers.tccd D-Bus interface."""
    cmd = ["busctl", "call", "--json=short", "com.tuxedocomputers.tccd", "/com/tuxedocomputers/tccd", "com.tuxedocomputers.tccd", method]
    if signature:
        cmd.append(signature)
        cmd.extend(str(a) for a in args)
    return run_cmd(cmd)


# ==============================================================================
# 1. Operating Modes
# ==============================================================================

def sync_oem_profiles() -> None:
    """Append missing defaults; preserve existing profile choices and user fields."""
    import stat
    import tempfile

    def snapshot():
        try:
            before = PROFILES_PATH.lstat()
        except FileNotFoundError:
            return None, None
        if not stat.S_ISREG(before.st_mode):
            raise ValueError(f"Refusing non-regular profile store: {PROFILES_PATH}")
        descriptor = os.open(PROFILES_PATH, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
                raise RuntimeError(f"Profile store changed while opening: {PROFILES_PATH}")
            contents = source.read()
            after = os.fstat(source.fileno())
        if identity(before) != identity(after):
            raise RuntimeError(f"Profile store changed while reading: {PROFILES_PATH}")
        return contents, after

    def identity(info):
        if info is None:
            return None
        return (info.st_dev, info.st_ino, info.st_uid, info.st_gid,
                stat.S_IMODE(info.st_mode), info.st_size, info.st_mtime_ns, info.st_ctime_ns)

    def unique_object(pairs):
        value = {}
        for key, entry in pairs:
            if key in value:
                raise ValueError(f"Duplicate JSON key in {PROFILES_PATH}: {key}")
            value[key] = entry
        return value

    def invalid_constant(value):
        raise ValueError(f"Invalid JSON constant in {PROFILES_PATH}: {value}")

    def profile_ids(records, origin):
        if not isinstance(records, list):
            raise ValueError(f"Expected profile list in {origin}")
        ids = set()
        for index, record in enumerate(records):
            if not isinstance(record, dict) or not isinstance(record.get("id"), str) or not record["id"].strip():
                raise ValueError(f"Invalid profile record at index {index} in {origin}")
            if record["id"] in ids:
                raise ValueError(f"Duplicate profile ID in {origin}: {record['id']}")
            ids.add(record["id"])
        return ids

    original, original_info = snapshot()
    if original is None:
        current_profiles = []
    else:
        current_profiles = json.loads(original.decode("utf-8"), object_pairs_hook=unique_object,
                                      parse_constant=invalid_constant)
    existing_ids = profile_ids(current_profiles, PROFILES_PATH)
    profile_ids(MECHREVO_PROFILES, "built-in defaults")
    missing = [record for record in MECHREVO_PROFILES if record["id"] not in existing_ids]
    if not missing:
        return  # No write, inode replacement, chmod, or mtime change for a complete store.

    payload = (json.dumps(current_profiles + missing, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")
    PROFILES_PATH.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=".mechrevo-profiles-", dir=PROFILES_PATH.parent)
    temporary = Path(temporary_name)
    committed = False
    try:
        with os.fdopen(descriptor, "wb") as staged:
            if original_info is not None:
                staged_info = os.fstat(staged.fileno())
                if (staged_info.st_uid, staged_info.st_gid) != (original_info.st_uid, original_info.st_gid):
                    os.fchown(staged.fileno(), original_info.st_uid, original_info.st_gid)
                os.fchmod(staged.fileno(), stat.S_IMODE(original_info.st_mode))
            else:
                os.fchmod(staged.fileno(), 0o644)
            staged.write(payload)
            staged.flush()
            os.fsync(staged.fileno())

        latest, latest_info = snapshot()
        if latest != original or identity(latest_info) != identity(original_info):
            raise RuntimeError(f"Profile store changed concurrently; refusing overwrite: {PROFILES_PATH}")
        if original_info is None:
            # link is an atomic create-if-absent; a newly appeared destination is never replaced.
            os.link(temporary, PROFILES_PATH)
        else:
            # Final observed byte/identity comparison is above. Non-cooperating writers can
            # still race in the narrow interval before replace; this is not kernel-level CAS.
            os.replace(temporary, PROFILES_PATH)
        committed = True
        directory = os.open(PROFILES_PATH.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except Exception as exc:
        if committed:
            raise RuntimeError(f"Profiles committed but directory durability check failed; inspect {PROFILES_PATH}: {exc}") from exc
        raise
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def send_notification(title: str, message: str, icon: str = "preferences-system-power") -> None:
    """Send desktop notification via notify-send with critical urgency to bypass DND."""
    try:
        subprocess.run(
            ["notify-send", "-u", "critical", "-t", "3000", "-a", "MECHREVO Gaming Center", "-i", icon, title, message],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except Exception:
        pass


def get_current_mode_info() -> tuple[str, str]:
    """Get active profile (name, id)."""
    try:
        out = call_tccd_dbus("GetActiveProfileJSON")
        parsed = json.loads(out)
        if "data" in parsed and parsed["data"]:
            raw_inner = parsed["data"][0]
            profile_data = json.loads(raw_inner) if isinstance(raw_inner, str) else raw_inner
            return profile_data.get("name", "Unknown"), profile_data.get("id", "")
    except Exception:
        pass
    return "默认模式", ""


def get_current_mode() -> str:
    """Get the active performance mode name."""
    return get_current_mode_info()[0]


def set_hardware_mode_led(mode_key: str) -> None:
    """Sync EC hardware mode register 0x0751 to update physical LED indicator."""
    raw_mode_map = {
        "office": 0xa0,
        "gaming": 0x00,
        "turbo": 0x10,
        "boost": 0x10,
    }
    if mode_key not in raw_mode_map:
        return
    val = raw_mode_map[mode_key]
    dev_path = Path("/dev/tuxedo_io")
    if dev_path.exists():
        try:
            fd = os.open(dev_path, os.O_RDWR)
            W_UW_MODE = 0x4008f012
            buf = struct.pack("q", val)
            fcntl.ioctl(fd, W_UW_MODE, buf)
            os.close(fd)
        except Exception:
            pass


def set_mode(mode_name: str, notify: bool = False) -> None:
    """Set the active performance mode (office, gaming, turbo, boost)."""
    sync_oem_profiles()
    mapping = {
        "office": ("mechrevo_office", "办公节能模式 (Office)", "长期功耗 PL1: 45W | 低噪静音温控曲线"),
        "gaming": ("mechrevo_gaming", "游戏平衡模式 (Gaming)", "长期功耗 PL1: 75W | 动态睿频温控曲线"),
        "turbo": ("mechrevo_turbo", "极速狂暴模式 (Turbo)", "长期功耗 PL1: 130W 满血释放 | 强冷温控曲线"),
        "boost": ("mechrevo_fan_boost", "一键全速强冷 (Fan Boost)", "双风扇 100% 满转速运行"),
    }
    mode_key = mode_name.lower().strip()
    if mode_key not in mapping:
        raise ValueError(f"Unknown mode '{mode_name}'. Valid options: {', '.join(mapping.keys())}")

    target_id, title_name, desc_info = mapping[mode_key]
    call_tccd_dbus("SetTempProfileById", "s", target_id)
    set_hardware_mode_led(mode_key)
    print(f"[✓] 成功切换至机械革命运行模式: {title_name} ({target_id})")

    if notify:
        send_notification("★ 机械革命电竞控制台 ★", f"运行模式已切换为：【{title_name}】\n{desc_info}")


def cycle_mode(notify: bool = True) -> str:
    """Cycle through Office -> Gaming -> Turbo -> Office and notify."""
    _, current_id = get_current_mode_info()
    order = ["mechrevo_office", "mechrevo_gaming", "mechrevo_turbo"]
    key_map = {
        "mechrevo_office": "office",
        "mechrevo_gaming": "gaming",
        "mechrevo_turbo": "turbo",
    }

    if current_id in order:
        idx = order.index(current_id)
        next_id = order[(idx + 1) % len(order)]
    else:
        # Default fallback to gaming
        next_id = "mechrevo_gaming"

    next_key = key_map[next_id]
    set_mode(next_key, notify=notify)
    return next_key


def toggle_touchpad() -> None:
    """Toggle touchpad via desktop script or omarchy tool."""
    script = Path.home() / ".config/hypr/scripts/toggle-touchpad.sh"
    if script.exists():
        run_cmd([str(script)], check=False)
    elif shutil.which("omarchy-toggle-touchpad"):
        run_cmd(["omarchy-toggle-touchpad"], check=False)
    elif shutil.which("omarchy-toggle-input-device"):
        run_cmd(["omarchy-toggle-input-device", "touchpad", "toggle"], check=False)


def run_daemon() -> None:
    """Listen to physical hotkey events (Mode button, Fn+Esc, Touchpad F21, Backlight)."""
    import struct
    import select

    dev_paths = [
        Path("/dev/input/by-path/platform-tuxedo_keyboard-event"),
    ]
    # Fallback to search /dev/input
    if not dev_paths[0].exists():
        for p in Path("/dev/input/by-path").glob("*tuxedo_keyboard*"):
            dev_paths.append(p)

    target_dev = None
    for p in dev_paths:
        if p.exists():
            target_dev = p
            break

    if not target_dev:
        print("错误: 找不到 tuxedo-keyboard 物理按键事件输入节点", file=sys.stderr)
        sys.exit(1)

    print(f"[*] 机械革命硬件热键守护进程已启动，监听设备: {target_dev}")
    last_mode_trigger = 0.0
    last_fn_trigger = 0.0
    last_tp_trigger = 0.0
    last_kbl_trigger = 0.0

    with open(target_dev, "rb") as f:
        # struct input_event: timeval (16B), type (2B), code (2B), value (4B) -> 24B on 64-bit
        event_format = "qqHHi"
        event_size = struct.calcsize(event_format)

        while True:
            data = f.read(event_size)
            if not data or len(data) < event_size:
                break
            _, _, ev_type, ev_code, ev_value = struct.unpack(event_format, data)
            # EV_KEY = 1, ev_value == 1 (key press)
            if ev_type == 1 and ev_value == 1:
                now = time.time()
                # 1. Physical mode switch key (KEY_F6 = 64)
                if ev_code == 64:
                    if now - last_mode_trigger >= 0.35:  # 350ms debounce
                        last_mode_trigger = now
                        print("[*] 捕捉到机身实体模式按键触发！执行模式切换...")
                        try:
                            cycle_mode(notify=True)
                        except Exception as err:
                            print(f"模式切换失败: {err}", file=sys.stderr)

                # 2. Fn Lock toggle key (KEY_FN_ESC = 465)
                elif ev_code == 465:
                    if now - last_fn_trigger >= 0.35:
                        last_fn_trigger = now
                        print("[*] 捕捉到 Fn+Esc (Fn 模式锁) 物理组合键触发！执行切换...")
                        try:
                            toggle_fn_lock(notify=True)
                        except Exception as err:
                            print(f"Fn Lock 切换失败: {err}", file=sys.stderr)

                # 3. Touchpad toggle key (KEY_F21 = 191)
                elif ev_code == 191:
                    if now - last_tp_trigger >= 0.4:
                        last_tp_trigger = now
                        print("[*] 捕捉到触控板硬件切换键 (KEY_F21) 触发！执行触控板开关...")
                        try:
                            toggle_touchpad()
                        except Exception as err:
                            print(f"触控板切换失败: {err}", file=sys.stderr)

                # 4. Keyboard backlight keys (KEY_KBDILLUMTOGGLE = 228, KEY_KBDILLUMDOWN = 229, KEY_KBDILLUMUP = 230)
                elif ev_code == 228:
                    if now - last_kbl_trigger >= 0.2:
                        last_kbl_trigger = now
                        if shutil.which("omarchy-brightness-keyboard"):
                            run_cmd(["omarchy-brightness-keyboard", "cycle"], check=False)
                elif ev_code == 229:
                    if now - last_kbl_trigger >= 0.1:
                        last_kbl_trigger = now
                        if shutil.which("omarchy-brightness-keyboard"):
                            run_cmd(["omarchy-brightness-keyboard", "down"], check=False)
                elif ev_code == 230:
                    if now - last_kbl_trigger >= 0.1:
                        last_kbl_trigger = now
                        if shutil.which("omarchy-brightness-keyboard"):
                            run_cmd(["omarchy-brightness-keyboard", "up"], check=False)


# ==============================================================================
# 2. Fan Boost / One-Click Full Speed
# ==============================================================================

def set_fan(action: str) -> None:
    """Control cooling fans (auto, boost, quiet)."""
    if action == "boost":
        set_mode("boost")
        print("[✓] 机械革命一键强冷 (Fan Boost) 已激活：双风扇已拉至 100% 满速强冷！")
    elif action == "auto":
        set_mode("gaming")
        print("[✓] 已恢复机械革命游戏智能温控自动模式。")
    elif action == "quiet":
        set_mode("office")
        print("[✓] 已切换至机械革命静音办公风扇模式。")
    else:
        raise ValueError(f"Unknown fan action '{action}'. Valid: auto, boost, quiet")


# ==============================================================================
# 3. Win Key Lock (电竞游戏防误触锁)
# ==============================================================================

def get_win_lock() -> bool:
    """Check if Win Key Lock is enabled."""
    f = get_state_file("win_lock")
    if f.exists():
        return f.read_text().strip() == "1"
    return False


def set_win_lock(enabled: bool) -> None:
    """Enable or disable Win Key lock via hardware AT scancode mapping."""
    # e05b is the scancode for Left Super/Windows key
    # 0 = disabled (no event emitted), 125 = KEY_LEFTMETA (standard)
    keycode = "0" if enabled else "125"
    run_cmd(["setkeycodes", "e05b", keycode])
    get_state_file("win_lock").write_text("1" if enabled else "0")
    state_str = "已锁定 (防误触生效，按 Win 键无响应)" if enabled else "已解锁 (正常使用)"
    print(f"[✓] 机械革命 Win 键防误触锁: {state_str}")


# ==============================================================================
# 4. Touchpad Hardware Lock
# ==============================================================================

def get_touchpad() -> bool:
    """Check if touchpad is enabled (device bound)."""
    return TOUCHPAD_BOUND_CHECK.exists()


def set_touchpad(enabled: bool) -> None:
    """Enable or disable touchpad via I2C driver bind/unbind."""
    current = get_touchpad()
    if enabled and not current:
        if TOUCHPAD_BIND_PATH.exists():
            TOUCHPAD_BIND_PATH.write_text(TOUCHPAD_I2C_DEVICE)
    elif not enabled and current:
        if TOUCHPAD_UNBIND_PATH.exists():
            TOUCHPAD_UNBIND_PATH.write_text(TOUCHPAD_I2C_DEVICE)
    get_state_file("touchpad_state").write_text("1" if enabled else "0")
    state_str = "已开启 (正常滑动)" if enabled else "已锁定禁用 (防误触生效)"
    print(f"[✓] 机械革命触控板状态: {state_str}")


# ==============================================================================
# 5. Fn Lock
# ==============================================================================

def get_fn_lock() -> bool:
    """Check if Fn Lock is enabled."""
    if FN_LOCK_PATH.exists():
        try:
            return FN_LOCK_PATH.read_text().strip() == "1"
        except Exception:
            pass
    return False


def set_fn_lock(enabled: bool, notify: bool = True) -> None:
    """Enable or disable Fn Lock."""
    if not FN_LOCK_PATH.exists():
        raise RuntimeError("Fn lock sysfs interface not available on this platform.")
    val_str = "1" if enabled else "0"
    try:
        FN_LOCK_PATH.write_text(val_str)
    except PermissionError:
        run_cmd(["sudo", "chmod", "0666", str(FN_LOCK_PATH)], check=False)
        try:
            FN_LOCK_PATH.write_text(val_str)
        except Exception:
            p = subprocess.Popen(["sudo", "tee", str(FN_LOCK_PATH)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            p.communicate(input=val_str.encode())

    # 1 = Multimedia priority, 0 = Standard F1-F12 priority
    mode_name = "多媒体快捷键优先" if enabled else "标准功能键优先 (F1-F12)"
    desc = "直接按 F1-F12 调节音量/亮度/触控板，按 Fn+Fx 触发标准功能键" if enabled else "直接按 F1-F12 触发标准功能键，按 Fn+Fx 调节多媒体"
    print(f"[✓] 机械革命 Fn 快捷键模式: {mode_name} ({desc})")

    if shutil.which("omarchy-osd"):
        run_cmd(["omarchy-osd", "-i", "keyboard", "-m", f"Fn 锁定: {'已开启 (多媒体)' if enabled else '已关闭 (F1-F12)'}"], check=False)

    if notify:
        send_notification("★ 机械革命键盘 ★", f"Fn 键模式已切换为：\n【{mode_name}】\n{desc}")


def toggle_fn_lock(notify: bool = True) -> bool:
    """Toggle Fn Lock state."""
    new_state = not get_fn_lock()
    set_fn_lock(new_state, notify=notify)
    return new_state



# ==============================================================================
# 6. Battery Health Care (3-Stage Charging)
# ==============================================================================

def get_battery_profile() -> str:
    """Get current battery charging profile."""
    if BATTERY_PROFILE_PATH.exists():
        return BATTERY_PROFILE_PATH.read_text().strip()
    return "high_capacity"


def set_battery_profile(profile: str) -> None:
    """Set battery profile: high (100%), balanced (80%), stationary (60%)."""
    mapping = {
        "high": "high_capacity",
        "high_capacity": "high_capacity",
        "balanced": "balanced",
        "stationary": "stationary",
        "long_life": "stationary",
    }
    p_key = profile.lower().strip()
    if p_key not in mapping:
        raise ValueError(f"Unknown battery profile '{profile}'. Valid: high, balanced, stationary")

    target = mapping[p_key]
    if BATTERY_PROFILE_PATH.exists():
        BATTERY_PROFILE_PATH.write_text(target)
    call_tccd_dbus("SetChargingProfile", "s", target)
    desc = {
        "high_capacity": "高容量模式 (充电至 100%)",
        "balanced": "平衡保养模式 (限充 80%)",
        "stationary": "长寿命保护模式 (限充 60%)",
    }[target]
    print(f"[✓] 机械革命电池健康保养模式: {desc}")


# ==============================================================================
# 7. RGB Keyboard Lighting
# ==============================================================================

def set_rgb(color_hex: str, brightness: int | None = None) -> None:
    """Set keyboard RGB color and brightness."""
    # Preset colors
    presets = {
        "red": (255, 0, 0),
        "green": (0, 255, 0),
        "blue": (0, 100, 255),
        "cyan": (0, 220, 220),
        "magenta": (255, 0, 200),
        "yellow": (255, 200, 0),
        "white": (255, 255, 255),
        "orange": (255, 100, 0),
        "purple": (160, 32, 240),
        "ice": (120, 200, 255),
    }

    c_key = color_hex.lower().lstrip("#")
    if c_key in presets:
        r, g, b = presets[c_key]
    elif len(c_key) == 6:
        r = int(c_key[0:2], 16)
        g = int(c_key[2:4], 16)
        b = int(c_key[4:6], 16)
    else:
        raise ValueError(f"Invalid color '{color_hex}'. Use HEX code (e.g. ff0000) or preset: {', '.join(presets.keys())}")

    multi_intensity_path = RGB_LEDS_DIR / "multi_intensity"
    brightness_path = RGB_LEDS_DIR / "brightness"

    if multi_intensity_path.exists():
        multi_intensity_path.write_text(f"{r} {g} {b}\n")

    if brightness is not None and brightness_path.exists():
        b_val = max(0, min(3, int(brightness)))
        brightness_path.write_text(f"{b_val}\n")

    print(f"[✓] 机械革命单区 RGB 灯效已更新: RGB({r}, {g}, {b})" + (f", 亮度: {brightness}/3" if brightness is not None else ""))


def apply_display_color(temp_k: int | None = None, gamma: float | None = None, is_identity: bool = False) -> None:
    """Apply screen color calibration safely (avoiding NVIDIA CTM grey screen)."""
    # X11 via xrandr
    if os.environ.get("DISPLAY") and shutil.which("xrandr"):
        try:
            g_str = f"{gamma:.2f}:{gamma:.2f}:{gamma:.2f}" if gamma else "1.0:1.0:1.0"
            run_cmd(["xrandr", "--output", "eDP-1", "--gamma", g_str], check=False)
        except Exception:
            pass


def set_screen_color_mode(mode_name: str, notify: bool = True) -> None:
    """Set screen color preset (standard, eyecare, gaming, movie)."""
    profiles = {
        "standard": {
            "name": "原彩标准模式 (Standard)",
            "desc": "标准 sRGB 还原 | 6500K 纯正色温 | Gamma 1.0",
            "temp": 6500,
            "gamma": 1.0,
            "identity": True,
        },
        "eyecare": {
            "name": "护眼防蓝光模式 (Eye Care)",
            "desc": "温润滤蓝光 | 4500K 暖色温 | 降低视觉疲劳",
            "temp": 4500,
            "gamma": 1.0,
            "identity": False,
        },
        "gaming": {
            "name": "电竞色彩增强模式 (Gaming)",
            "desc": "电竞暗部提亮 | 6800K 高清晰色温 | Gamma 1.15",
            "temp": 6800,
            "gamma": 1.15,
            "identity": False,
        },
        "movie": {
            "name": "影音沉浸模式 (Movie)",
            "desc": "高对比浓郁色彩 | 6000K 影视色温 | Gamma 1.05",
            "temp": 6000,
            "gamma": 1.05,
            "identity": False,
        },
    }

    m_key = mode_name.lower().strip()
    if m_key not in profiles:
        raise ValueError(f"Unknown color mode '{mode_name}'. Valid: {', '.join(profiles.keys())}")

    p = profiles[m_key]
    apply_display_color(temp_k=p["temp"], gamma=p["gamma"], is_identity=p["identity"])
    get_state_file("color_profile").write_text(m_key)
    print(f"[✓] 机械革命屏幕色彩调教: {p['name']} ({p['desc']})")

    if notify:
        send_notification("★ 机械革命电竞控制台 ★", f"屏幕色彩已切换为：【{p['name']}】\n{p['desc']}", icon="video-display")


def get_screen_color_mode() -> str:
    """Get current screen color profile."""
    f = get_state_file("color_profile")
    if f.exists():
        c_id = f.read_text().strip()
        names = {
            "standard": "原彩标准模式 (6500K)",
            "eyecare": "护眼防蓝光模式 (4500K)",
            "gaming": "电竞色彩增强模式 (6800K)",
            "movie": "影音沉浸模式 (6000K)",
        }
        return names.get(c_id, c_id)
    return "原彩标准模式 (Standard)"


def set_screen_brightness(percent_str: str) -> None:
    """Set screen display brightness using brightnessctl."""
    val = percent_str.strip().rstrip("%")
    try:
        p_val = max(1, min(100, int(val)))
        run_cmd(["brightnessctl", "set", f"{p_val}%"])
        print(f"[✓] 屏幕显示亮度已设置为: {p_val}%")
    except Exception as e:
        raise RuntimeError(f"设置亮度失败: {e}")


# ==============================================================================
# 9. Real-Time Telemetry & Dashboard
# ==============================================================================

def read_hwmon_sensors() -> dict:
    """Read temperatures and fan RPM from mechrevo_ec_sensors and coretemp."""
    data = {"cpu_temp": "N/A", "gpu_temp": "N/A", "fan1_rpm": "0", "fan2_rpm": "0"}
    hwmon_base = Path("/sys/class/hwmon")
    if hwmon_base.exists():
        for hw in hwmon_base.glob("hwmon*"):
            name_file = hw / "name"
            if name_file.exists():
                name = name_file.read_text().strip()
                if name in ("mechrevo_ec", "mechrevo_ec_sensors"):
                    # fan1: CPU fan, fan2: GPU fan
                    f1 = hw / "fan1_input"
                    f2 = hw / "fan2_input"
                    t1 = hw / "temp1_input"
                    t2 = hw / "temp2_input"
                    if f1.exists():
                        data["fan1_rpm"] = f1.read_text().strip()
                    if f2.exists():
                        data["fan2_rpm"] = f2.read_text().strip()
                    if t1.exists():
                        data["cpu_temp"] = f"{int(t1.read_text().strip()) / 1000:.1f} °C"
                    if t2.exists():
                        data["gpu_temp"] = f"{int(t2.read_text().strip()) / 1000:.1f} °C"
                elif name == "coretemp" and data["cpu_temp"] == "N/A":
                    pkg = hw / "temp1_input"
                    if pkg.exists():
                        data["cpu_temp"] = f"{int(pkg.read_text().strip()) / 1000:.1f} °C"
    return data


def show_dashboard() -> None:
    """Display modern Mechrevo Gaming Center status card."""
    sensors = read_hwmon_sensors()
    mode = get_current_mode()
    win_lock = "已锁定 (防误触)" if get_win_lock() else "未锁定 (正常)"
    touchpad = "已启用" if get_touchpad() else "已禁用"
    fn_lock = "F1-F12 优先" if get_fn_lock() else "多媒体快捷键优先"
    battery = {
        "high_capacity": "100% 高容量模式",
        "balanced": "80% 平衡保养模式",
        "stationary": "60% 长寿保护模式",
    }.get(get_battery_profile(), get_battery_profile())

    color_mode = get_screen_color_mode()

    card = f"""
===================================================================
        ★ 机械革命电竞控制台 (MECHREVO Gaming Center) ★
===================================================================
【运行性能模式】:  {mode}
【屏幕色彩调教】:  {color_mode}
【系统散热监控】:
  • CPU 核心温度:  {sensors['cpu_temp']}
  • GPU 核心温度:  {sensors['gpu_temp']}
  • CPU 散热风扇:  {sensors['fan1_rpm']} RPM
  • 独显散热风扇:  {sensors['fan2_rpm']} RPM

【电竞专属开关】:
  • Win 键防误触:  {win_lock}
  • 笔记本触控板:  {touchpad}
  • Fn 键锁定状态: {fn_lock}
  • 电池健康保养:  {battery}
===================================================================
提示:
  切换模式:  mechrevo-control mode [office|gaming|turbo|cycle]
  屏幕调色:  mechrevo-control color [standard|eyecare|gaming|movie]
  屏幕亮度:  mechrevo-control brightness [10-100%]
  一键强冷:  mechrevo-control fan boost (恢复: fan auto)
  防误触锁:  mechrevo-control win-lock [on|off|toggle]
  触控板锁:  mechrevo-control touchpad [on|off|toggle]
  背光调节:  mechrevo-control rgb [cyan|red|blue|#HEX] [0-3]
===================================================================
"""
    print(card)


def main() -> int:
    parser = argparse.ArgumentParser(description="机械革命电竞控制台 Linux 核心管理工具")
    subparsers = parser.add_subparsers(dest="command", help="子命令")

    # mode
    p_mode = subparsers.add_parser("mode", help="切换或查看运行模式 (office, gaming, turbo, boost, cycle)")
    p_mode.add_argument("target", nargs="?", choices=["office", "gaming", "turbo", "boost", "cycle"], help="目标模式")
    p_mode.add_argument("--notify", action="store_true", help="发送桌面通知")

    # color
    p_color = subparsers.add_parser("color", help="屏幕色彩调教 (standard, eyecare, gaming, movie)")
    p_color.add_argument("target", nargs="?", choices=["standard", "eyecare", "gaming", "movie"], help="色彩预设")

    # brightness
    p_bright = subparsers.add_parser("brightness", help="屏幕显示亮度设置 (1-100%%)")
    p_bright.add_argument("percent", help="亮度百分比 (例如 80%% 或 80)")

    # daemon
    subparsers.add_parser("daemon", help="启动机身模式物理按键监听守护进程")

    # fan
    p_fan = subparsers.add_parser("fan", help="风扇控制 (auto, boost, quiet)")
    p_fan.add_argument("action", choices=["auto", "boost", "quiet"], help="风扇操作")

    # win-lock
    p_win = subparsers.add_parser("win-lock", help="Win 键防误触锁控制")
    p_win.add_argument("action", nargs="?", choices=["on", "off", "toggle", "status"], default="status")

    # touchpad
    p_tp = subparsers.add_parser("touchpad", help="笔记本触控板硬件开关")
    p_tp.add_argument("action", nargs="?", choices=["on", "off", "toggle", "status"], default="status")

    # fn-lock
    p_fn = subparsers.add_parser("fn-lock", help="Fn 快捷键锁控制")
    p_fn.add_argument("action", nargs="?", choices=["on", "off", "toggle", "status"], default="status")

    # battery
    p_bat = subparsers.add_parser("battery", help="电池健康保养模式 (high, balanced, stationary)")
    p_bat.add_argument("profile", nargs="?", choices=["high", "balanced", "stationary"], help="保养模式")

    # rgb
    p_rgb = subparsers.add_parser("rgb", help="键盘 RGB 背光与亮度调节")
    p_rgb.add_argument("color", help="RGB 十六进制颜色代码或预设名称 (red, cyan, blue, etc.)")
    p_rgb.add_argument("brightness", nargs="?", type=int, choices=[0, 1, 2, 3], help="背光亮度 (0-3)")

    # status / dashboard
    subparsers.add_parser("status", help="查看电竞控制台系统仪表盘")

    # sync
    subparsers.add_parser("sync-profiles", help="向控制中心注入机械革命原厂 Profiles")

    args = parser.parse_args()

    if not args.command or args.command == "status":
        show_dashboard()
        return 0

    try:
        if args.command == "mode":
            if args.target == "cycle":
                cycle_mode(notify=True)
            elif args.target:
                set_mode(args.target, notify=args.notify)
            else:
                print(f"当前运行模式: {get_current_mode()}")
        elif args.command == "color":
            if args.target:
                set_screen_color_mode(args.target, notify=True)
            else:
                print(f"当前屏幕色彩模式: {get_screen_color_mode()}")
        elif args.command == "brightness":
            set_screen_brightness(args.percent)
        elif args.command == "daemon":
            run_daemon()
        elif args.command == "fan":
            set_fan(args.action)
        elif args.command == "win-lock":
            if args.action == "on":
                set_win_lock(True)
            elif args.action == "off":
                set_win_lock(False)
            elif args.action == "toggle":
                set_win_lock(not get_win_lock())
            else:
                print(f"Win 键锁状态: {'已锁定 (防误触生效)' if get_win_lock() else '未锁定 (正常)'}")
        elif args.command == "touchpad":
            if args.action == "on":
                set_touchpad(True)
            elif args.action == "off":
                set_touchpad(False)
            elif args.action == "toggle":
                set_touchpad(not get_touchpad())
            else:
                print(f"触控板状态: {'已启用' if get_touchpad() else '已禁用'}")
        elif args.command == "fn-lock":
            if args.action == "on":
                set_fn_lock(True, notify=True)
            elif args.action == "off":
                set_fn_lock(False, notify=True)
            elif args.action == "toggle":
                toggle_fn_lock(notify=True)
            else:
                mode = "多媒体快捷键优先" if get_fn_lock() else "标准功能键优先 (F1-F12)"
                print(f"Fn 键模式: {mode}")
        elif args.command == "battery":
            if args.profile:
                set_battery_profile(args.profile)
            else:
                print(f"当前电池保养模式: {get_battery_profile()}")
        elif args.command == "rgb":
            set_rgb(args.color, args.brightness)
        elif args.command == "sync-profiles":
            sync_oem_profiles()
            print("[✓] 机械革命原厂 Profiles 已成功注入 /etc/tcc/profiles")
        return 0
    except Exception as e:
        print(f"错误: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
