"""Execute driver functions with failing firmware/kernel-service substitutes.

No modules, EC writes or privileged operations are used. The C functions and
their constants are extracted from the source under review, not reimplemented.
DRIVER_QUALITY_SOURCE_ROOT can select an older checkout for sensitivity checks.
"""
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    # Keep the newer signature of the two supported probe/remove API variants.
    source = re.sub(
        r"#if LINUX_VERSION_CODE[^\n]*\n([^\n]*\b(?:probe|remove)\([^\n]*\))"
        r"\n#else\n([^\n]+)\n#endif", r"\2", source,
    )
    match = re.search(
        r"(?m)^(?:static )?(?:int|void|bool|ssize_t|enum led_brightness)\s+"
        r"(?:__init |__exit )?" + re.escape(name) + r"\([^;{]*\)\s*\{", source,
    )
    if not match:
        raise AssertionError("Missing function: " + name)
    depth, position = 1, match.end()
    while depth and position < len(source):
        depth += (source[position] == "{") - (source[position] == "}")
        position += 1
    if depth:
        raise AssertionError("Unbalanced function: " + name)
    return source[match.start():position]


class DriverQualityRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="driver-quality-")
        cls.addClassCleanup(cls.temp.cleanup)
        path = Path(cls.temp.name)
        selected = Path(os.environ.get("DRIVER_QUALITY_SOURCE_ROOT", ROOT))
        sources = {
            name: (selected / "src" / name).read_text()
            for name in ("clevo_wmi.c", "clevo_acpi.c", "clevo_keyboard.h",
                         "clevo_leds.h", "uniwill_keyboard.h", "tuxedo_io/tuxedo_io.c")
        }
        interfaces = (selected / "src/clevo_interfaces.h").read_text()
        constants = "\n".join(re.findall(r"(?m)^#define CLEVO_\w+[^\n]*$", interfaces))
        constants += "\n" + "\n".join(re.findall(
            r"(?m)^#define (?:CLEVO_KBD_|CLEVO_KB_COLOR_|CLEVO_KB_MODE_|UNIWILL_LIGHTBAR_|UW_MEMORY_OVERCLOCKING_|UW_CPU_PERFORMANCE_AND_OVERCLOCKING_)\w+[^\n]*$",
            sources["clevo_leds.h"] + sources["clevo_keyboard.h"] + sources["uniwill_keyboard.h"],
        ))
        cls.binaries = {}
        for group in ("transport", "clevo", "lifecycle"):
            fixture = (ROOT / f"tests/fixtures/driver_quality_{group}.c").read_text()
            fixture = fixture.replace("/* @CONSTANTS@ */", constants)

            def insert(match):
                filename, name = match.groups()
                return function(sources[filename], name)

            fixture = re.sub(r"/\* @FUNCTION ([\w./]+) (\w+)@ \*/", insert, fixture)
            if group == "clevo":
                led = sources["clevo_leds.h"]
                enum = re.search(r"enum clevo_kb_backlight_types\s*\{.*?\};", led, re.S).group()
                names = ("clevo_led_cdev", "clevo_mcled_cdevs_subleds", "clevo_mcled_cdevs")
                declarations = [re.search(
                    r"(?ms)^static struct [^;{]*\b" + name + r"(?:\[[^\]]+\])* = \{.*?^\};", led,
                ).group() for name in names]
                keyboard = sources["clevo_keyboard.h"]
                for name in ("kbd_led_state_t", "kbd_backlight_mode_t"):
                    declarations.append(re.search(
                        r"(?ms)^static struct " + name + r" \{.*?^\};", keyboard,
                    ).group())
                declarations.append(re.search(
                    r"(?ms)^static const struct kernel_param_ops param_ops_mode_ops = \{.*?^\};", keyboard,
                ).group())
                fixture = fixture.replace("/* @LED_ENUM@ */", enum)
                fixture = fixture.replace("/* @DECLARATIONS@ */", "\n".join(declarations))
            elif group == "lifecycle":
                keyboard = sources["uniwill_keyboard.h"]
                old_helpers = []
                for name in ("uniwill_read_lightbar_rgb", "uniwill_read_lightbar_animation"):
                    if re.search(r"(?m)^static void " + name, keyboard):
                        old_helpers.append(function(keyboard, name))
                fixture = fixture.replace("/* @OLD_LIGHTBAR_READERS@ */", "\n".join(old_helpers))
            source, binary = path / f"{group}.c", path / group
            source.write_text(fixture)
            result = subprocess.run(
                ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                 "-Wno-unused-parameter", "-Wno-unused-function", "-Wno-unused-variable",
                 "-Wno-unused-but-set-variable", "-Wno-sign-compare",
                 str(source), "-o", str(binary)], text=True, capture_output=True,
            )
            if result.returncode:
                raise AssertionError(result.stdout + result.stderr)
            cls.binaries[group] = binary

    def check_case(self, group, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run(
            [str(self.binaries[group]), case], capture_output=True, text=True,
            timeout=10, preexec_fn=no_core,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


CASES = {
    "transport": (
        "wmi_error_cleanup", "wmi_absent_result", "wmi_discard_result", "wmi_transfer_result",
        "wmi_invalid_events", "wmi_valid_event", "wmi_unsupported_package",
        "acpi_discard_result", "acpi_transfer_result", "acpi_failures", "acpi_event_identity",
        "probe_alloc_failure", "probe_notify_failure", "probe_binding_failure", "probe_success",
    ),
    "clevo": (
        "fn_get_valid", "fn_get_invalid", "fn_get_failure", "fn_set_invalid_read",
        "fn_set_success", "fn_availability", "parameter_width", "mode_invalid",
        "mode_failure", "mode_success", "legacy_read_failure", "legacy_probe_failure",
        "led_spec_short", "led_spec_success", "led_spec_retry", "led_spec_reset",
        "led_brightness_error", "led_rgb_error", "led_notify_failure", "led_notify_success",
        "battery_empty_registration", "bind_failure", "bind_success", "bind_duplicate",
        "bind_wrong_remove", "bind_priority", "combo_no_input",
    ),
    "lifecycle": (
        "io_alloc_failure", "io_cdev_failure", "io_class_failure", "io_device_failure", "io_success",
        "lightbar_channel_reads", "lightbar_read_failure", "lightbar_animation_bits",
        "lightbar_animation_failure", "lightbar_color_failure", "lightbar_invalid",
        "lightbar_init_failure", "lightbar_init_success", "efi_short_second_read", "efi_minimum_valid_read",
    ),
}


def test_case(group, case):
    def run(self):
        self.check_case(group, case)
    return run


for group, cases in CASES.items():
    for case in cases:
        setattr(DriverQualityRuntime, "test_" + case, test_case(group, case))


if __name__ == "__main__":
    unittest.main()
