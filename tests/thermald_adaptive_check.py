#!/usr/bin/env python3
"""Build selected pinned thermald adaptive functions with explicit service doubles.

No daemon, hardware, GLib IPC or real libevdev object runs. Actual source cleanup
closes isolated ordinary file descriptors. GPL source provenance is checked before
applying the maintained patch; an optional prepared source may be supplied.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess

from test_thermald_ceiling_runtime import COMMON, SYSFS, actual_functions

SELECTED = {
    'thd_sensor.cpp': ['cthd_sensor::cthd_sensor', 'cthd_sensor::read_temperature'],
    'thd_engine.cpp': ['cthd_engine::get_sensor_temperature'],
    'thd_gddv.cpp': [
        'cthd_gddv::gddv_free', 'cthd_gddv::destroy_dynamic_sources',
        'cthd_gddv::compare_condition', 'cthd_gddv::compare_time',
        'cthd_gddv::evaluate_temperature_condition',
        'cthd_gddv::evaluate_condition', 'cthd_gddv::evaluate_condition_set',
        'cthd_gddv::evaluate_conditions',
    ],
    'thd_engine_adaptive.cpp': [
        'cthd_engine_adaptive::update_engine_state',
        'cthd_engine_adaptive::thd_engine_start',
    ],
}


def verify_sources(workspace):
    fixture = workspace / 'tests/fixtures/thermald_adaptive'
    provenance = json.loads((fixture / 'provenance.json').read_text())
    if provenance['commit'] != '5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a' or provenance['license'] != 'GPL-2.0-or-later':
        raise AssertionError('Unexpected upstream qualification or license')
    sources = {}
    for name, expected in provenance['sha256'].items():
        sources[name] = fixture / name
        if hashlib.sha256(sources[name].read_bytes()).hexdigest() != expected:
            raise AssertionError('Frozen GPL adaptive source changed: ' + name)
    sensors = workspace / 'tests/fixtures/thermald_checked'
    sensor_provenance = json.loads((sensors / 'provenance.json').read_text())
    if sensor_provenance['commit'] != provenance['commit']:
        raise AssertionError('Adaptive and base sensor upstream commits differ')
    for name, expected in provenance['sensor_sha256'].items():
        sources[name] = sensors / name
        if hashlib.sha256(sources[name].read_bytes()).hexdigest() != expected:
            raise AssertionError('Frozen GPL sensor source changed: ' + name)
    return sources


def prepare_source(workspace, output, checked=True):
    """Restore pinned upstream source and apply the real maintained patch subset."""
    source = output / 'src'
    source.mkdir(parents=True, exist_ok=True)
    sources = verify_sources(workspace)
    for name, path in sources.items():
        shutil.copy2(path, source / name)
    if checked:
        subprocess.run(['git', 'init', '-q', str(output)], check=True,
                       capture_output=True, text=True, timeout=10,
                       preexec_fn=disable_core, env={**os.environ, 'TMPDIR': str(output)})
        patch = workspace / 'packaging/arch/thermald-mechrevo/0001-checked-temperature.patch'
        command = ['git', 'apply'] + ['--include=src/' + name for name in sources]
        result = subprocess.run(command + [str(patch)], cwd=output,
                                capture_output=True, text=True, timeout=10,
                                preexec_fn=disable_core, env={**os.environ, 'TMPDIR': str(output)})
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        if 'read_temperature_checked' not in (source / 'thd_sensor.h').read_text():
            raise AssertionError('Maintained checked patch did not apply')
    return source


def build(workspace, source, output, mutation=None):
    output.mkdir(parents=True, exist_ok=True)
    (output / 'thd_sensor.h').write_bytes((source / 'thd_sensor.h').read_bytes())
    (output / 'thd_common.h').write_text(COMMON + '\n#define thd_log_error(...) fake_log(__VA_ARGS__)\n#define ARRAY_SIZE(x) (sizeof(x)/sizeof((x)[0]))\n')
    fake = SYSFS.replace('    int failure = -EIO;',
                         '    int failure = -EIO;\n    int read_error = 0;\n    bool empty_read = false;\n    unsigned read_attempts = 0;')
    fake = fake.replace('        auto found = state().numbers.find(key);',
                        '        ++state().read_attempts;\n        if (state().empty_read) return 0;\n        if (state().read_error) return state().read_error;\n        auto found = state().numbers.find(key);')
    fake = fake.replace('    fake_state &state() {',
                        '    void update_path(std::string p) { path=std::move(p); }\n    fake_state &state() {')
    (output / 'thd_sys_fs.h').write_text(fake)
    (output / 'thd_util.h').write_text('#pragma once\ninline bool starts_with(const std::string&a,const std::string&b){return a.rfind(b,0)==0;}\n')
    header = (source / 'thd_gddv.h').read_text()
    declarations = []
    for anchor in ['enum adaptive_condition :', 'enum adaptive_comparison :',
                   'enum adaptive_operation :', 'struct condition {']:
        start = header.index(anchor)
        declarations.append(header[start:header.index('};', start) + 2])
    functions = []
    bodies = []
    for name, selected in SELECTED.items():
        text = (source / name).read_text()
        methods = list(selected)
        if name == 'thd_sensor.cpp' and 'read_temperature_checked' in text:
            methods.append('cthd_sensor::read_temperature_checked')
        for method in methods:
            body = actual_functions(text, method).decode()
            functions.append({'file': name, 'function': method,
                              'sha256': hashlib.sha256(body.encode()).hexdigest()})
            if mutation == 'no-cleanup' and method == 'cthd_engine_adaptive::thd_engine_start':
                needle = '{ gddv.gddv_free(); return THD_ERROR; }'
                if body.count(needle) != 2:
                    raise AssertionError('Expected two checked startup cleanup branches')
                body = body.replace(needle, '{ return THD_ERROR; }')
            if mutation == 'sentinel-no-match' and method == 'cthd_gddv::evaluate_conditions':
                needle = 'return THD_SENSOR_READ_ERROR; // Caller preserves current matched target.'
                if body.count(needle) != 1:
                    raise AssertionError('Expected one checked condition sentinel return')
                body = body.replace(needle, 'return THD_ERROR; // Deliberate regression mutant.')
            bodies.append(body)
    template = (workspace / 'tests/fixtures/thermald_adaptive/runtime.cpp').read_text()
    code = template.replace('/* @CONDITION_DECLARATIONS@ */', '\n'.join(declarations))
    code = code.replace('/* @ACTUAL_FUNCTIONS@ */', '\n'.join(bodies))
    cpp = output / 'runtime.cpp'
    cpp.write_text(code)
    binary = output / 'runtime'
    result = subprocess.run([
        'c++', '-std=c++17', '-O2', '-Wall', '-Wextra', '-Werror',
        '-Wno-unused-parameter', '-I' + str(output), str(cpp), '-o', str(binary),
    ], capture_output=True, text=True, timeout=30,
        env={**os.environ, 'TMPDIR': str(output)}, preexec_fn=disable_core)
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    return binary, functions


def disable_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def run(binary, case, output):
    output.mkdir(parents=True, exist_ok=True)
    return subprocess.run([str(binary), case, str(output)], capture_output=True,
                          text=True, timeout=10, preexec_fn=disable_core,
                          env={**os.environ, 'TMPDIR': str(output)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--source-dir', type=Path)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    disable_core()
    workspace, output = args.workspace.resolve(), args.out.resolve()
    verify_sources(workspace)
    source = args.source_dir.resolve() if args.source_dir else prepare_source(workspace, output)
    binary, functions = build(workspace, source, output / 'build')
    rows = []
    for case in ['invalid', 'short-circuit', 'startup-unsupported',
                 'startup-supported', 'valid-no-match']:
        result = run(binary, case, output / case)
        rows.append({'case': case, 'exit_code': result.returncode,
                     'stdout': result.stdout, 'stderr': result.stderr})
    receipt = {'scope': 'Actual selected sensor/engine/GDDV/adaptive functions. '
               'Class storage, unrelated predicates, policy effects, GLib/libevdev '
               'objects are doubles. Actual cleanup closes ordinary FDs, including '
               'alias ownership. Not full daemon startup or hardware execution.',
               'functions': functions, 'cases': rows}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
    return int(any(row['exit_code'] for row in rows))


if __name__ == '__main__':
    raise SystemExit(main())
