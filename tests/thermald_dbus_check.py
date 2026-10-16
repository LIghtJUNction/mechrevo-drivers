#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Pinned actual DBus providers and two GDBus handler branches, isolated.

Sensor/engine/providers/handler bodies are selected verbatim from qualified GPL
sources. Allocation, GVariant, IPC and engine storage are explicit doubles; null
allocation results are deterministic fault injection, not a claim that production
GLib g_new recovers from OOM. No daemon, bus, hardware or dependency download runs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import subprocess

from test_thermald_ceiling_runtime import COMMON, SYSFS, actual_functions

PIN = '5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a'


def disable_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def verify_sources(workspace):
    result = {}
    for directory, names in [
        ('thermald_checked', ['thd_sensor.h', 'thd_sensor.cpp']),
        ('thermald_virtual', ['thd_engine.cpp', 'thd_dbus_interface.cpp']),
    ]:
        fixture = workspace / 'tests/fixtures' / directory
        provenance = json.loads((fixture / 'provenance.json').read_text())
        if provenance['commit'] != PIN or provenance['license'] != 'GPL-2.0-or-later':
            raise AssertionError('Unexpected upstream DBus qualification or license')
        for name in names:
            source = fixture / name
            if hashlib.sha256(source.read_bytes()).hexdigest() != provenance['sha256'][name]:
                raise AssertionError('Frozen GPL DBus source changed: ' + name)
            result[name] = source
    return result


def prepare_source(workspace, output, checked=True):
    source = output / 'src'
    source.mkdir(parents=True, exist_ok=True)
    sources = verify_sources(workspace)
    for name, path in sources.items():
        shutil.copy2(path, source / name)
    if checked:
        environment = {**os.environ, 'TMPDIR': str(output)}
        subprocess.run(['git', 'init', '-q', str(output)], check=True,
                       capture_output=True, text=True, timeout=10,
                       preexec_fn=disable_core, env=environment)
        patch = workspace / 'packaging/arch/thermald-mechrevo/0001-checked-temperature.patch'
        result = subprocess.run(['git', 'apply'] + ['--include=src/' + name for name in sources] + [str(patch)],
                                cwd=output, capture_output=True, text=True, timeout=10,
                                preexec_fn=disable_core, env=environment)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        if 'read_temperature_checked' not in (source / 'thd_sensor.h').read_text():
            raise AssertionError('Maintained checked patch did not apply')
    return source


def handler_branch(text, method):
    """Select one exact brace-balanced branch; this is not a C++/DBus parser."""
    pattern = r'if\s*\(g_strcmp0\(method_name,\s*"' + re.escape(method) + r'"\)\s*==\s*0\)\s*\{'
    matches = list(re.finditer(pattern, text))
    if len(matches) != 1:
        raise AssertionError('Expected one actual GDBus branch: ' + method)
    match = matches[0]
    end, depth = match.end(), 1
    while depth and end < len(text):
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    if depth:
        raise AssertionError('Unbalanced actual GDBus branch: ' + method)
    return text[match.start():end]


PRELUDE = r'''
#include <iostream>
#include <set>
#include <type_traits>
#include "thd_sensor.h"
static void require(bool ok,const char*message){if(!ok){std::cerr<<message<<"\n";std::exit(1);}}
class cthd_engine {
public:
 std::vector<std::unique_ptr<cthd_sensor>> sensors;
 cthd_sensor*user_get_sensor(int i){return i>=0&&i<(int)sensors.size()?sensors[i].get():nullptr;}
 int get_sensor_temperature(int,unsigned int*);
};
static cthd_engine engine;static cthd_engine*thd_engine=&engine;
using gboolean=bool;using gint=int;using guint=unsigned int;using gchar=char;
static_assert(std::is_signed<gint>::value&&!std::is_signed<guint>::value,"GLib integer signedness changed");
struct PrefObject{};struct GError{};
#define TRUE true
#define FALSE false
/* @REPLY_LIMIT@ */
static int allocation_attempts=0,allocated=0,freed=0,allocation_fail_after=-1;
static std::set<void*>owned;
template<class T>T*tracked_new(size_t n){
 ++allocation_attempts;if(allocation_fail_after==0)return nullptr;if(allocation_fail_after>0)--allocation_fail_after;
 auto*p=static_cast<T*>(std::malloc(n*sizeof(T)));require(p!=nullptr,"test backing allocation failed");
 std::memset(p,0xa5,n*sizeof(T));owned.insert(p);++allocated;return p;
}
#define g_new(type,n) tracked_new<type>(n)
static void g_free(void*p){if(p){require(owned.erase(p)==1,"DBus freed foreign or already released memory");++freed;std::free(p);}}
static void fixture_auto_free(gchar**p){g_free(*p);}
#define g_autofree __attribute__((cleanup(fixture_auto_free)))
struct Parameters{guint index=0;};
struct Variant{std::string signature,sensor,path;gint signed_temperature=0;guint unsigned_temperature=0;};
struct Invocation{std::unique_ptr<Variant>response;int responses=0,errors=0;std::string error_method;};
static int g_strcmp0(const char*a,const char*b){return std::strcmp(a,b);}
static void g_variant_get(Parameters*p,const char*format,gint*out){require(std::string(format)=="(u)","sensor request index must retain unsigned wire signature");*out=static_cast<gint>(p->index);}
static Variant*g_variant_new(const char*format,const char*sensor,const char*path,gint temp){
 require(std::string(format)=="(ssi)","information response must retain (ssi) wire signature");
 auto*v=new Variant;v->signature=format;v->sensor=sensor;v->path=path;v->signed_temperature=temp;return v;
}
static Variant*g_variant_new(const char*format,guint temp){
 require(std::string(format)=="(u)","temperature response must retain (u) wire signature");
 auto*v=new Variant;v->signature=format;v->unsigned_temperature=temp;return v;
}
static void g_dbus_method_invocation_return_value(Invocation*i,Variant*v){++i->responses;i->response.reset(v);}
static void fixture_return_error(Invocation*i,const char*method){++i->errors;i->error_method=method;}
#define return_dbus_error(method) fixture_return_error(invocation,method)
'''

TESTS = r'''
static void reset_allocations(){require(owned.empty(),"prior DBus case retained ownership");allocation_attempts=allocated=freed=0;allocation_fail_after=-1;}
static void provider_failure(int fail_after,const char*read_kind){
 reset_allocations();auto&state=*fake_files.at(engine.sensors[0]->get_sensor_path());
 state.read_error=std::string(read_kind)=="io"?-EIO:0;state.empty_read=std::string(read_kind)=="eof";
 state.numbers["temp"]=std::string(read_kind)=="negative"?-1:95000;allocation_fail_after=fail_after;
 gchar*sensor=reinterpret_cast<char*>(1);gchar*path=reinterpret_cast<char*>(2);gint temp=123456;
 GError*error=reinterpret_cast<GError*>(3);const auto reads=state.read_attempts;
 const bool ok=thd_dbus_interface_get_sensor_information(nullptr,0,&sensor,&path,&temp,&error);
 require(!ok,fail_after<0?"DBus information must reject invalid temperature":"DBus information must reject allocation failure");
 require(sensor==reinterpret_cast<char*>(1)&&path==reinterpret_cast<char*>(2)&&temp==123456&&error==reinterpret_cast<GError*>(3),"DBus failure must preserve caller outputs");
 if(fail_after<0){
  require(owned.empty()&&allocation_attempts==2&&allocated==2&&freed==2,"DBus failed read must release both allocated strings");
  require(state.read_attempts==reads+1,"DBus failed read must use actual sensor read");
 }else{
  require(owned.empty()&&allocation_attempts==fail_after+1&&allocated==fail_after&&freed==fail_after,"DBus allocation failure must release owned temporary strings");
  require(state.read_attempts==reads,"DBus allocation failure must not read sensor");
 }
}
static void provider_success(int temperature){
 reset_allocations();auto&state=*fake_files.at(engine.sensors[0]->get_sensor_path());state.numbers["temp"]=temperature;
 gchar*sensor=nullptr;gchar*path=nullptr;gint temp=-99;GError*error=nullptr;
 require(thd_dbus_interface_get_sensor_information(nullptr,0,&sensor,&path,&temp,&error),"DBus valid information rejected");
 require(temp==temperature&&error==nullptr,"DBus valid signed temperature changed");
 require(owned.size()==2&&allocated==2&&freed==0&&owned.count(sensor)&&owned.count(path)&&sensor!=path,"DBus successful strings must transfer distinct ownership to caller");
 require(std::string(sensor)==engine.sensors[0]->get_sensor_type().substr(0,MAX_DBUS_REPLY_STR_LEN)&&std::string(path)==engine.sensors[0]->get_sensor_path().substr(0,MAX_DBUS_REPLY_STR_LEN),"DBus successful strings must retain bounded terminated content");
 g_free(sensor);g_free(path);require(owned.empty()&&freed==2,"DBus caller-owned strings did not release exactly once");
 guint unsigned_temp=123456;require(thd_dbus_interface_get_sensor_temperature(nullptr,0,&unsigned_temp,&error)&&unsigned_temp==static_cast<guint>(temperature),"DBus unsigned temperature provider changed");
}
static void handler_success(int temperature){
 reset_allocations();fake_files.at(engine.sensors[0]->get_sensor_path())->numbers["temp"]=temperature;
 Parameters p;Invocation info;dispatch("GetSensorInformation",&p,&info);
 require(info.responses==1&&info.errors==0&&info.response&&info.response->signature=="(ssi)"&&info.response->signed_temperature==temperature&&info.response->sensor=="A"&&info.response->path=="a/","actual information handler response payload changed");
 require(owned.empty()&&allocated==2&&freed==2,"actual information handler must automatically release both successful strings");
 Invocation temp;dispatch("GetSensorTemperature",&p,&temp);
 require(temp.responses==1&&temp.errors==0&&temp.response&&temp.response->signature=="(u)"&&temp.response->unsigned_temperature==static_cast<guint>(temperature),"actual temperature handler response payload changed");
}
static void handler_failures(){
 Parameters p;
 for(int fail_after:{0,1,-1}){
  reset_allocations();allocation_fail_after=fail_after;
  auto&state=*fake_files.at("a/");state.read_error=fail_after<0?-EIO:0;
  Invocation info;dispatch("GetSensorInformation",&p,&info);
  require(info.responses==0&&info.errors==1&&info.error_method=="GetSensorInformation"&&!info.response,"failed information handler must return error without payload");
  require(owned.empty()&&allocated==freed,"failed information handler retained temporary ownership");
 }
 reset_allocations();fake_files.at("a/")->read_error=-EIO;
 Invocation temp;dispatch("GetSensorTemperature",&p,&temp);
 require(temp.responses==0&&temp.errors==1&&temp.error_method=="GetSensorTemperature"&&!temp.response,"failed temperature handler must return error without payload");
}
static void invalid_index(){
 reset_allocations();gchar*sensor=reinterpret_cast<char*>(1);gchar*path=reinterpret_cast<char*>(2);gint temp=123456;GError*error=nullptr;
 for(int index:{-1,1}){
  require(!thd_dbus_interface_get_sensor_information(nullptr,index,&sensor,&path,&temp,&error),"DBus invalid sensor index accepted");
  require(sensor==reinterpret_cast<char*>(1)&&path==reinterpret_cast<char*>(2)&&temp==123456&&allocation_attempts==0,"DBus invalid index changed outputs or allocated memory");
  guint unsigned_temp=123456;require(!thd_dbus_interface_get_sensor_temperature(nullptr,index,&unsigned_temp,&error)&&unsigned_temp==123456,"DBus invalid index changed unsigned temperature output");
 }
}
int main(int argc,char**argv){
 require(argc==2,"fixture case required");const std::string name=argv[1];
 const bool long_strings=name=="long-strings";
 engine.sensors.emplace_back(new cthd_sensor(7,long_strings?std::string(150,'p')+"/":"a/",long_strings?std::string(150,'s'):"A"));
 fake_files.at(engine.sensors[0]->get_sensor_path())->numbers["temp"]=95000;
 if(name=="io"||name=="eof"||name=="negative")provider_failure(-1,name.c_str());
 else if(name=="allocation-first")provider_failure(0,"valid");
 else if(name=="allocation-second")provider_failure(1,"valid");
 else if(name=="normal"||name=="long-strings")provider_success(95000);
 else if(name=="zero")provider_success(0);
 else if(name=="handler-normal")handler_success(95000);
 else if(name=="handler-zero")handler_success(0);
 else if(name=="handler-failures")handler_failures();
 else if(name=="invalid-index")invalid_index();
 else require(false,"unknown fixture case");
 std::cout<<name<<": passed\n";
}
'''


def build(workspace, source, output, mutation=None):
    output.mkdir(parents=True, exist_ok=True)
    (output / 'thd_sensor.h').write_bytes((source / 'thd_sensor.h').read_bytes())
    (output / 'thd_common.h').write_text(COMMON + '\n#define thd_log_error(...) fake_log(__VA_ARGS__)\n')
    fake = SYSFS.replace('    int failure = -EIO;', '    int failure = -EIO;\n    int read_error = 0;\n    bool empty_read = false;\n    unsigned read_attempts = 0;')
    fake = fake.replace('        auto found = state().numbers.find(key);', '        ++state().read_attempts;\n        if (state().empty_read) return 0;\n        if (state().read_error) return state().read_error;\n        auto found = state().numbers.find(key);')
    fake = fake.replace('    fake_state &state() {', '    void update_path(std::string p) { path=std::move(p); }\n    fake_state &state() {')
    (output / 'thd_sys_fs.h').write_text(fake)
    (output / 'thd_util.h').write_text('#pragma once\ninline bool starts_with(const std::string&a,const std::string&b){return a.rfind(b,0)==0;}\n')
    dbus = (source / 'thd_dbus_interface.cpp').read_text()
    limits = re.findall(r'^#define MAX_DBUS_REPLY_STR_LEN\s+\d+\s*$', dbus, re.MULTILINE)
    if len(limits) != 1:
        raise AssertionError('Expected actual bounded DBus reply length definition')
    code = PRELUDE.replace('/* @REPLY_LIMIT@ */', limits[0])
    selected = {'thd_sensor.cpp': ['cthd_sensor::cthd_sensor', 'cthd_sensor::read_temperature'],
                'thd_engine.cpp': ['cthd_engine::get_sensor_temperature'],
                'thd_dbus_interface.cpp': ['thd_dbus_interface_get_sensor_information', 'thd_dbus_interface_get_sensor_temperature']}
    functions = []
    for name, methods in selected.items():
        text = (source / name).read_text()
        methods = list(methods)
        if name == 'thd_sensor.cpp' and 'read_temperature_checked' in text:
            methods.append('cthd_sensor::read_temperature_checked')
        for method in methods:
            body = actual_functions(text, method).decode()
            functions.append({'file': name, 'function': method, 'sha256': hashlib.sha256(body.encode()).hexdigest()})
            if method == 'thd_dbus_interface_get_sensor_information':
                if mutation == 'read-leak':
                    old = 'g_free(sensor_str);\n        g_free(path_str);'
                    if body.count(old) != 1:
                        raise AssertionError('Expected one checked read cleanup block')
                    body = body.replace(old, 'g_free(sensor_str); /* deliberate path leak */')
                if mutation == 'second-allocation-leak':
                    old = 'if (!path_str) {\n\t\tg_free(sensor_str);'
                    if body.count(old) != 1:
                        raise AssertionError('Expected one second-allocation cleanup block')
                    body = body.replace(old, 'if (!path_str) {\n\t\t/* deliberate first-string leak */')
            code += '\n' + body + '\n'
    code += '\nstatic void dispatch(const char*method_name,Parameters*parameters,Invocation*invocation){GError*error=nullptr;PrefObject object;PrefObject*obj=&object;\n'
    for method in ['GetSensorInformation', 'GetSensorTemperature']:
        body = handler_branch(dbus, method)
        functions.append({'file': 'thd_dbus_interface.cpp', 'branch': method, 'sha256': hashlib.sha256(body.encode()).hexdigest()})
        if mutation == 'response-type' and method == 'GetSensorInformation':
            if body.count('g_variant_new("(ssi)"') != 1:
                raise AssertionError('Expected one information response constructor')
            body = body.replace('g_variant_new("(ssi)"', 'g_variant_new("(ssu)"')
        code += body + '\n'
    code += 'require(false,"unknown GDBus method");}\n' + TESTS
    cpp = output / 'runtime.cpp'
    cpp.write_text(code)
    binary = output / 'runtime'
    result = subprocess.run(['c++', '-std=c++17', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter', '-I' + str(output), str(cpp), '-o', str(binary)],
                            capture_output=True, text=True, timeout=30, preexec_fn=disable_core,
                            env={**os.environ, 'TMPDIR': str(output)})
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    return binary, functions


def run(binary, case):
    return subprocess.run([str(binary), case], capture_output=True, text=True,
                          timeout=10, preexec_fn=disable_core,
                          env={**os.environ, 'TMPDIR': str(binary.parent)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--source-dir', type=Path)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    workspace, output = args.workspace.resolve(), args.out.resolve()
    disable_core()
    verify_sources(workspace)
    source = args.source_dir.resolve() if args.source_dir else prepare_source(workspace, output)
    binary, functions = build(workspace, source, output / 'build')
    cases = []
    for case in ['io', 'eof', 'negative', 'allocation-first', 'allocation-second',
                 'zero', 'normal', 'long-strings', 'handler-zero', 'handler-normal',
                 'handler-failures', 'invalid-index']:
        result = run(binary, case)
        cases.append({'case': case, 'exit_code': result.returncode,
                      'stdout': result.stdout, 'stderr': result.stderr})
    receipt = {'scope': __doc__, 'functions': functions, 'cases': cases}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
    return int(any(case['exit_code'] for case in cases))


if __name__ == '__main__':
    raise SystemExit(main())
