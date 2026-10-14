#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile the actual Mini LED initializer with small kernel/EC test doubles.

Run with ``python3 tests/test_uniwill_mini_led_init.py``. To check another
revision with the same harness, set UNIWILL_KEYBOARD_HEADER to its header path.
"""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


HEADER = Path(os.environ.get(
    "UNIWILL_KEYBOARD_HEADER",
    Path(__file__).resolve().parents[1] / "src" / "uniwill_keyboard.h",
))


def initializer_source():
    source = HEADER.read_text()
    start = source.index("static void uw_mini_led_local_dimming_init(")
    opening = source.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


FIXTURE = r"""
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

struct platform_device { struct { int kobj; } dev; };
struct uniwill_device_features_t { bool uniwill_has_mini_led_local_dimming; };
static struct uniwill_device_features_t uniwill_device_features;
static bool uw_mini_led_local_dimming_loaded;
static const int uw_mini_led_local_dimming_attr_group;
static int registration_result, registrations, writes;
static bool last_value = true, write_before_registration;

static int sysfs_create_group(int *kobj, const int *group)
{
    (void)kobj;
    (void)group;
    ++registrations;
    return registration_result;
}

static int uw_set_mini_led_local_dimming(bool value)
{
    write_before_registration = registrations != 1 || registration_result != 0;
    ++writes;
    last_value = value;
    return 0;
}

/* The function under test is extracted verbatim from the selected header. */
@INITIALIZER@

int main(int argc, char **argv)
{
    struct platform_device device = {0};
    int scenario;
    if (argc != 2)
        return 2;
    scenario = atoi(argv[1]);
    uniwill_device_features.uniwill_has_mini_led_local_dimming = scenario != 0;
    registration_result = scenario == 1 ? -12 : 0;
    uw_mini_led_local_dimming_init(&device);
    if (registrations != (scenario != 0) || writes != (scenario == 2) ||
        uw_mini_led_local_dimming_loaded != (scenario == 2) ||
        (scenario == 2 && last_value) || write_before_registration) {
        fprintf(stderr, "scenario=%d registrations=%d writes=%d loaded=%d value=%d\n",
                scenario, registrations, writes,
                uw_mini_led_local_dimming_loaded, last_value);
        return 1;
    }
    return 0;
}
"""


class MiniLedInitializationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="uniwill-mini-led-")
        cls.addClassCleanup(cls.temporary.cleanup)
        temporary = Path(cls.temporary.name)
        source = temporary / "mini_led_init.c"
        source.write_text(FIXTURE.replace("@INITIALIZER@", initializer_source()))
        cls.binary = temporary / "mini_led_init"
        subprocess.run([
            os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra",
            "-Werror", str(source), "-o", str(cls.binary),
        ], check=True)

    def run_scenario(self, scenario):
        result = subprocess.run(
            [str(self.binary), str(scenario)], capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unsupported_panel_never_registers_or_writes(self):
        self.run_scenario(0)

    def test_failed_sysfs_registration_never_writes(self):
        self.run_scenario(1)

    def test_supported_panel_registers_then_initializes_to_off(self):
        self.run_scenario(2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
