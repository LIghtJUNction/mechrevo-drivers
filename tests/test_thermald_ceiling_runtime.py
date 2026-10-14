#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Execute pinned thermald 2.5.13 RAPL and sequential-trip C++ functions.

This fixture owns no hardware file descriptors. Class declarations, controller
functions and trip selection are exact GPL upstream snapshots. Sysfs, PPCC
discovery, power-meter observations, engine/sensor services, exit registration
and the next cooling device use doubles; PID/shutdown paths abort if entered.
Direct parameter setters model the native XML range application, including
unsafe counterexamples that the CLI must reject. The XML parser, daemon startup,
exit restoration, PID and physical thermal behavior are not executed here.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT = ROOT / 'tests/fixtures/thermald_v2_5_13'


def actual_functions(source, name):
    """Select every exact definition, including overloaded getters."""
    pattern = (r'(?m)^[^\n;{}]*' + re.escape(name)
               + r'\([^;{}]*?\)\s*(?::[^{}]*?)?\{')
    pieces = []
    for match in re.finditer(pattern, source):
        end, depth = match.end(), 1
        while depth and end < len(source):
            depth += (source[end] == '{') - (source[end] == '}')
            end += 1
        if depth:
            raise AssertionError('Unbalanced actual upstream function: ' + name)
        pieces.append(source[match.start():end])
    if not pieces:
        raise AssertionError('Missing actual upstream function: ' + name)
    return '\n\n'.join(pieces).encode()

COMMON = r'''
#pragma once
#include <algorithm>
#include <cerrno>
#include <climits>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include <vector>
#define THD_SUCCESS 0
#define THD_ERROR (-1)
template<class... Args> inline void fake_log(const char *,Args...) {}
#define thd_log_info(...) fake_log(__VA_ARGS__)
#define thd_log_warn(...) fake_log(__VA_ARGS__)
#define thd_log_debug(...) fake_log(__VA_ARGS__)
'''
SYSFS = r'''
#pragma once
#include "thd_common.h"
struct fake_write { std::string key; int value; int status; };
struct fake_state {
    std::map<std::string,int> numbers = {
        {"constraint_0_max_power_uw",55000000},
        {"constraint_0_power_limit_uw",140000000},
        {"constraint_0_time_window_us",55967744},
        {"constraint_1_power_limit_uw",140000000},
        {"constraint_1_time_window_us",2440}, {"enabled",1}};
    std::map<std::string,std::string> strings = {
        {"name","package-0"}, {"constraint_0_name","long_term"},
        {"constraint_1_name","short_term"}};
    std::vector<fake_write> writes;
    std::string fail_key;
    int failure = -EIO;
};
inline std::map<std::string,std::shared_ptr<fake_state>> fake_files;
class csys_fs {
    std::string path;
public:
    explicit csys_fs(std::string p = "") : path(std::move(p)) {
        if (!fake_files.count(path)) fake_files[path] = std::make_shared<fake_state>();
    }
    fake_state &state() { return *fake_files.at(path); }
    const std::string &get_base_path() const { return path; }
    bool exists(const std::string &key) {
        return state().numbers.count(key) || state().strings.count(key);
    }
    template<class T> int read(const std::string &key,T *out) {
        auto found = state().numbers.find(key);
        if (found == state().numbers.end()) return -ENOENT;
        *out = static_cast<T>(found->second);
        return static_cast<int>(std::to_string(found->second).size());
    }
    int read(const std::string &key,std::string &out) {
        auto found = state().strings.find(key);
        if (found == state().strings.end()) return -ENOENT;
        out = found->second; return static_cast<int>(out.size());
    }
    int write(const std::string &key,int value) {
        const int result = key == state().fail_key ? state().failure
                                                 : static_cast<int>(std::to_string(value).size());
        state().writes.push_back({key,value,result});
        if (result > 0) state().numbers[key] = value;
        return result;
    }
    int write(const std::string &,const char *) { std::abort(); }
};
'''
PID = r'''
#pragma once
enum pid_mode_t { PID_ABSOLUTE, PID_INCREMENTAL };
struct pid_param_t {
    int valid;
    double kp,ki,kd;
    pid_mode_t mode;
};
class cthd_pid {
public:
    double kp = 0,ki = 0,kd = 0;
    void reset() { std::abort(); }
    void set_target_temp(int) { std::abort(); }
    int pid_output(int,int) { std::abort(); }
    void set_pid_param(double,double,double) { std::abort(); }
    void set_pid_mode(pid_mode_t) { std::abort(); }
    pid_mode_t get_pid_mode() { std::abort(); }
};
'''


class ThermaldCeilingRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        manifest = json.loads((SNAPSHOT / 'provenance.json').read_text())
        for name, item in manifest['files'].items():
            digest = hashlib.sha256((SNAPSHOT / name).read_bytes()).hexdigest()
            if digest != item['snapshot_sha256']:
                raise AssertionError('Pinned upstream snapshot changed: ' + name)
            for function, expected in item.get('functions', {}).items():
                selected = actual_functions((SNAPSHOT / name).read_text(), function)
                if hashlib.sha256(selected).hexdigest() != expected:
                    raise AssertionError('Pinned function hash changed: ' + function)
        # Optional live frozen checkout verification adds exact whole-source
        # checks without making the ordinary repository suite depend on it.
        upstream = os.environ.get('THERMALD_PINNED_SOURCE_DIR')
        if upstream:
            for name, item in manifest['files'].items():
                actual = (Path(upstream) / item['upstream_path']).read_bytes()
                if hashlib.sha256(actual).hexdigest() != item['upstream_sha256']:
                    raise AssertionError('Frozen source mismatch: ' + name)
                for function, expected in item.get('functions', {}).items():
                    selected = actual_functions(actual.decode(), function)
                    snapshot = actual_functions((SNAPSHOT / name).read_text(), function)
                    if selected != snapshot or hashlib.sha256(selected).hexdigest() != expected:
                        raise AssertionError('Function differs from official source: ' + function)
        (ROOT / '.local-tests').mkdir(exist_ok=True)
        cls.temporary = tempfile.TemporaryDirectory(prefix='thermald-ceiling-',
                                                    dir=ROOT / '.local-tests')
        cls.addClassCleanup(cls.temporary.cleanup)
        path = Path(cls.temporary.name)
        headers = {
            'thd_common.h': COMMON,
            'thd_sys_fs.h': SYSFS,
            'thd_pid.h': PID,
            'thd_adaptive_types.h': '#pragma once\nstruct adaptive_target { '
                                    'std::string code; std::string argument; };\n',
        }
        for name, content in headers.items():
            (path / name).write_text(content)
        fixture = (ROOT / 'tests/fixtures/thermald_ceiling_runtime.cpp').read_text()
        for marker, name in (('/* @ACTUAL_CDEV@ */', 'thd_cdev.cpp'),
                             ('/* @ACTUAL_RAPL@ */', 'thd_cdev_rapl.cpp'),
                             ('/* @ACTUAL_TRIP@ */', 'thd_trip_point.cpp')):
            if fixture.count(marker) != 1:
                raise AssertionError('Actual-source insertion marker count: ' + marker)
            fixture = fixture.replace(marker, (SNAPSHOT / name).read_text())
        source = path / 'actual-thermald.cpp'
        source.write_text(fixture)
        cls.binary = path / 'actual-thermald'
        result = subprocess.run([os.environ.get('CXX', 'c++'), '-std=c++17', '-O2',
                                 '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter',
                                 '-I' + str(path), '-I' + str(SNAPSHOT), str(source),
                                 '-o', str(cls.binary)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def check_case(self, case):
        def no_core():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        result = subprocess.run([str(self.binary), case], capture_output=True,
                                text=True, timeout=10, preexec_fn=no_core)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_actual_initialisation_retains_native_140_to_70_w_range_and_step(self):
        self.check_case('native_range')

    def test_minimum_only_counterexample_blocks_further_thermal_cooling(self):
        self.check_case('min_only_counterexample')

    def test_paired_range_allows_40_w_and_releases_to_55_w_enabled(self):
        self.check_case('thermal_lower_release')

    def test_actual_controller_clamps_both_paired_range_boundaries(self):
        self.check_case('range_boundaries')

    def test_unmodified_native_policy_releases_to_its_own_140_w_maximum(self):
        self.check_case('original_policy')

    def test_failed_write_preserves_hardware_and_requires_independent_readback(self):
        self.check_case('write_failure')

    def test_bios_locked_controller_stops_writing_after_enodata(self):
        self.check_case('bios_locked')

    def test_invalid_maximum_counterexample_bypasses_ceiling_and_disables_rapl(self):
        self.check_case('invalid_max_restore')

    def test_initially_disabled_counterexample_loses_limit_when_thermal_trip_releases(self):
        self.check_case('initially_disabled')

    def test_actual_sequential_trip_escalates_to_next_cooling_device_at_paired_floor(self):
        self.check_case('sequential_next_device')

    def test_minimum_only_counterexample_blocks_actual_sequential_next_cooling_device(self):
        self.check_case('sequential_min_only_counterexample')


if __name__ == '__main__':
    unittest.main()
