#!/usr/bin/env python3
"""Execute driver functions with controlled firmware and kernel API doubles.

The C bodies are extracted from the repository at test time. These tests do not
load modules or access EC, ACPI, WMI, sysfs, or any real hardware.
"""
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).with_name("fixtures") / "platform_quality"
GCC = shutil.which("gcc")


def source(path):
    return (ROOT / path).read_text()


def function(text, name):
    """Read a definition, skipping compatibility directives before its body."""
    pattern = re.compile(
        r"(?:static\s+)?(?:int|void|bool|ssize_t|umode_t|u8)\s+"
        r"(?:__init\s+)?" + re.escape(name) +
        r"\s*\([^;{}#]*\)\s*(?:#[^\n]*\n\s*)*\{"
    )
    match = pattern.search(text)
    if not match:
        raise AssertionError(f"missing C definition: {name}")
    opening = match.end() - 1
    # Ignore braces in comments and string/character literals.
    tokens = re.compile(r'/\*.*?\*/|//[^\n]*|"(?:\\.|[^"\\])*"|'
                        r"'(?:\\.|[^'\\])*'|[{}]", re.S)
    depth = 1
    for token in tokens.finditer(text, opening + 1):
        if token.group() == "{":
            depth += 1
        elif token.group() == "}":
            depth -= 1
            if not depth:
                signature = re.sub(r"^\s*#[^\n]*\n", "", text[match.start():opening],
                                   flags=re.M)
                return signature + text[opening:token.end()] + "\n"
    raise AssertionError(f"unterminated C definition: {name}")


def functions(text, *names):
    return "\n".join(function(text, name) for name in names)


def struct(text, name):
    match = re.search(r"struct " + re.escape(name) + r"\s*\{[^}]*\};", text)
    if not match:
        raise AssertionError(f"missing struct: {name}")
    return match.group() + "\n"


def defines(text):
    return "\n".join(line for line in text.splitlines()
                     if line.startswith("#define ") and
                     not line.startswith("#define pr_fmt")) + "\n"


@unittest.skipUnless(GCC, "gcc is required for the C behavior harnesses")
class PlatformDriverQualityTests(unittest.TestCase):
    def run_c(self, name, body):
        with tempfile.TemporaryDirectory(prefix="mechrevo-driver-quality-") as temp:
            path = Path(temp) / f"{name}.c"
            path.write_text('#include "kernel_stubs.h"\n' + body)
            binary = Path(temp) / name
            built = subprocess.run(
                [GCC, "-std=gnu11", "-O2", "-Wall", "-Werror",
                 "-Wno-unused-function", "-Wno-unused-variable",
                 "-pthread", "-I", str(FIXTURES), str(path), "-o", str(binary)],
                capture_output=True, text=True, timeout=30)
            self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
            result = subprocess.run([str(binary)], capture_output=True,
                                    text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_nb04_wmi_payload_cleanup_and_device_removal(self):
        for variant in ("ab", "bs"):
            with self.subTest(variant=variant):
                text = source(f"src/tuxedo_nb04/tuxedo_nb04_wmi_{variant}.c")
                header = source(f"src/tuxedo_nb04/tuxedo_nb04_wmi_{variant}.h")
                if variant == "ab":
                    names = ("nb04_wmi_ab_evaluate", "nb04_wmi_ab_method_buffer",
                             "nb04_wmi_ab_method_buffer_reduced_output",
                             "nb04_wmi_ab_method_extended_input",
                             "nb04_wmi_ab_method_int_out",
                             "tuxedo_nb04_wmi_ab_remove")
                    lock = "nb04_wmi_ab_lock"
                    call = "nb04_wmi_ab_method_buffer"
                    remove = "tuxedo_nb04_wmi_ab_remove"
                    extra = r"""
    expected_length = 10;
    assert(nb04_wmi_ab_method_buffer_reduced_output(2, input, output) == 0);
    assert(last_input_length == 8 && output[9] == 0x3a);
    expected_length = 80;
    assert(nb04_wmi_ab_method_extended_input(2, input, output) == 0);
    assert(last_input_length == 496);
    integer_reply = true;
    u64 integer = 999;
    assert(nb04_wmi_ab_method_int_out(3, input, &integer) == 0);
    assert(integer == 42);
    integer_reply = false;
    assert(nb04_wmi_ab_method_int_out(3, input, &integer) == -EIO);
    assert(integer == 42);
"""
                else:
                    names = ("nb04_wmi_bs_available", "nb04_wmi_bs_method",
                             "tuxedo_nb04_wmi_remove")
                    lock = "nb04_wmi_bs_access_lock"
                    call = "nb04_wmi_bs_method"
                    remove = "tuxedo_nb04_wmi_remove"
                    extra = r"""
    assert(nb04_wmi_bs_available());
    integer_reply = true;
    assert(CALL(2, input, output) == -EINVAL);
    integer_reply = false;
"""
                harness = (FIXTURES / "wmi_test.c").read_text()
                generated = (defines(header) +
                             f"#define CALL {call}\n#define REMOVE {remove}\n" +
                             f"static DEFINE_MUTEX({lock});\n" +
                             "static struct wmi_device *__wmi_dev;\n")
                generated += harness.replace("@FUNCTIONS@", functions(text, *names))
                generated = generated.replace("@EXTRA@", extra)
                self.run_c(f"wmi_{variant}", generated)

    def test_tuxi_acpi_argument_and_output_bounds(self):
        text = source("src/tuxedo_tuxi/tuxi_acpi.c")
        names = ("evaluate_intparams", "tuxi_evaluate",
                 "tuxi_set_fan_speed", "tuxi_get_fan_speed", "tuxi_get_nr_fans",
                 "tuxi_set_fan_mode", "tuxi_get_fan_mode", "tuxi_get_fan_type",
                 "tuxi_get_fan_temp", "tuxi_get_fan_rpm", "tuxi_platform_remove")
        body = (struct(text, "tuxi_platform_driver_data_t") +
                "static DEFINE_MUTEX(tuxi_lock);\n"
                "static struct tuxi_platform_driver_data_t *tuxi_driver_data;\n")
        body += (FIXTURES / "tuxi_acpi_test.c").read_text().replace(
            "@FUNCTIONS@", functions(text, *names))
        self.run_c("tuxi_acpi", body)

    def test_tuxi_fan_read_errors_modes_and_present_channels(self):
        text = source("src/tuxedo_tuxi/tuxedo_tuxi_fan_control.c")
        names = tuple(f"fan{fan}_{kind}_{action}"
                      for fan in (1, 2) for kind in ("pwm", "pwm_enable")
                      for action in ("show", "store"))
        names += ("fan_control_is_visible", "hwm_is_visible", "hwm_read",
                  "hwm_read_string")
        labels = text[text.index("static const char * const hwm_temp_labels"):
                      text.index("static int hwm_read_string")]
        body = defines(text) + struct(text, "driver_data_t") + labels
        body += (FIXTURES / "tuxi_fan_test.c").read_text().replace(
            "@FUNCTIONS@", functions(text, *names))
        self.run_c("tuxi_fan", body)

    def test_nb04_profile_cache_commits_only_successful_writes(self):
        text = source("src/tuxedo_nb04/tuxedo_nb04_power_profiles.c")
        header = source("src/tuxedo_nb04/tuxedo_nb04_wmi_bs.h")
        options = text[text.index("struct char_to_value_t"):
                       text.index("static ssize_t platform_profile_choices_show",
                                  text.index("struct char_to_value_t"))]
        enums = "\n".join(re.findall(r"enum \w+\s*\{[^}]*\};", header)) + "\n"
        body = defines(header) + enums + defines(text) + struct(text, "driver_data_t") + options
        body += (FIXTURES / "nb04_profile_test.c").read_text().replace(
            "@FUNCTIONS@", functions(text, "set_system_mode",
                                    "platform_profile_choices_show",
                                    "platform_profile_show",
                                    "platform_profile_store",
                                    "tuxedo_nb04_power_profiles_probe"))
        self.run_c("nb04_profile", body)

    def test_nb05_profile_notifications_stop_and_cache(self):
        text = source("src/tuxedo_nb05/tuxedo_nb05_power_profiles.c")
        # This section contains the actual locks, delayed-work declaration,
        # firmware transactions, callbacks, options and sysfs handlers.
        code = text[text.index("static DEFINE_MUTEX"):
                    text.index("static DEVICE_ATTR_RO")]
        code += function(text, "stop_profile_work")
        body = (FIXTURES / "nb05_profile_test.c").read_text().replace(
            "@FUNCTIONS@", code)
        self.run_c("nb05_profile", body)

    def test_nb05_backlight_registration_bounds_and_removal(self):
        text = source("src/tuxedo_nb05/tuxedo_nb05_kbd_backlight.c")
        names = ("nb05_leds_set_brightness", "nb05_leds_notify_brightness_change_extern",
                 "init_leds", "tuxedo_nb05_kbd_backlight_probe",
                 "tuxedo_nb05_kbd_backlight_remove")
        declarations = text[text.index("static DEFINE_MUTEX"):
                            text.index("static int nb05_leds_set_brightness")]
        body = defines(text) + declarations
        body += (FIXTURES / "nb05_led_test.c").read_text().replace(
            "@FUNCTIONS@", functions(text, *names))
        self.run_c("nb05_led", body)

    def test_nb05_fan_version_boundary_and_input_validation(self):
        text = source("src/tuxedo_nb05/tuxedo_nb05_fan_control.c")
        header = source("src/tuxedo_nb05/tuxedo_nb05_ec.h")
        names = ("read_fan1_duty_ranges", "read_fan1_duty_onereg",
                 "read_fan2_duty", "write_fan1_enable_ranges",
                 "write_fan1_enable_onereg", "write_fan2_enable",
                 "fan1_pwm_show", "fan2_pwm_show",
                 "fan1_pwm_enable_store", "fan2_pwm_enable_store",
                 "tuxedo_nb05_fan_control_probe")
        body = (defines(text) + struct(header, "nb05_device_data_t") +
                struct(header, "nb05_ec_data_t") + struct(text, "driver_data_t"))
        body += (FIXTURES / "nb05_fan_test.c").read_text().replace(
            "@FUNCTIONS@", functions(text, *names))
        self.run_c("nb05_fan", body)

    def test_keyboard_malformed_payload_and_brightness_narrowing(self):
        for model in ("nb04", "nb05"):
            with self.subTest(model=model):
                text = source(f"src/tuxedo_{model}/tuxedo_{model}_keyboard.c")
                body = defines(text) + struct(text, "driver_data_t")
                fixture = (FIXTURES / "keyboard_test.c").read_text()
                fixture = fixture.replace("@NOTIFY@", f"tuxedo_{model}_keyboard_notify")
                fixture = fixture.replace("@FUNCTIONS@",
                                          function(text, f"tuxedo_{model}_keyboard_notify"))
                fixture = fixture.replace("@NB05@", "1" if model == "nb05" else "0")
                self.run_c(f"keyboard_{model}", body + fixture)


if __name__ == "__main__":
    unittest.main()
