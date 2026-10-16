# SPDX-License-Identifier: GPL-2.0-or-later
#!/usr/bin/env python3
"""Actual thermald periodic callback regression; explicit paths, no hardware/daemon."""
import argparse,hashlib,json,os,resource,subprocess,sys
from pathlib import Path
ap=argparse.ArgumentParser()
ap.add_argument('--workspace',type=Path,required=True)
ap.add_argument('--source-dir',type=Path,required=True)
ap.add_argument('--out',type=Path,required=True)
a=ap.parse_args();sys.path.insert(0,str(a.workspace/'tests'))
from test_thermald_ceiling_runtime import COMMON,SYSFS,actual_functions
out=a.out.resolve();out.mkdir(parents=True,exist_ok=True)
resource.setrlimit(resource.RLIMIT_CORE,(0,0))
for name in ['thd_sensor.h','thd_sensor_virtual.h']:(out/name).write_bytes((a.source_dir/name).read_bytes())
(out/'thd_common.h').write_text(COMMON+'\n#define thd_log_error(...) fake_log(__VA_ARGS__)\n')
sf=SYSFS.replace('    int failure = -EIO;','    int failure = -EIO;\n    int read_error = 0;\n    bool empty_read = false;')
sf=sf.replace('        auto found = state().numbers.find(key);','        if (state().empty_read) return 0;\n        if (state().read_error) return state().read_error;\n        auto found = state().numbers.find(key);')
sf=sf.replace('    fake_state &state() {','    void update_path(std::string p) { path = std::move(p); }\n    fake_state &state() {')
(out/'thd_sys_fs.h').write_text(sf)
(out/'thd_util.h').write_text('#pragma once\ninline bool starts_with(const std::string&a,const std::string&b){return a.rfind(b,0)==0;}\n')
code=r'''#include <atomic>
#include <thread>
#include <mutex>
#include <memory>
#include <functional>
#include <vector>
#include <cmath>
#include <algorithm>
#include <climits>
#include <iostream>
#include "thd_sensor.h"
#define private public
#include "thd_sensor_virtual.h"
#undef private
static void require(bool ok,const char*m){if(!ok){std::cerr<<m<<"\n";std::exit(1);}}
struct fake_engine {std::map<std::string,cthd_sensor*> named;cthd_sensor* search_sensor(const std::string&n){auto i=named.find(n);return i==named.end()?nullptr:i->second;}};
static fake_engine engine;static fake_engine*thd_engine=&engine;
std::mutex cthd_sensor_virtual::dependency_mutex;
void cthd_sensor_virtual::disable_periodic_timer(){require(!polling.load(),"fixture cache mode left running");}
static std::function<void(unsigned)> on_sleep;
static unsigned controlled_sleep(unsigned value){on_sleep(value);return 0;}
'''
methods={'thd_sensor.cpp':['cthd_sensor::cthd_sensor','cthd_sensor::read_temperature','cthd_sensor::read_temperature_checked'], 'thd_sensor_virtual.cpp':['cthd_sensor_virtual::cthd_sensor_virtual','cthd_sensor_virtual::~cthd_sensor_virtual','cthd_sensor_virtual::add_target','cthd_sensor_virtual::dependency_reaches','cthd_sensor_virtual::update_polling_table','cthd_sensor_virtual::read_temperature','cthd_sensor_virtual::read_temperature_checked','cthd_sensor_virtual::_read_temperature','cthd_sensor_virtual::_read_temperature_checked','periodic_callback']}
functions=[]
for name,names in methods.items():
 text=(a.source_dir/name).read_text()
 for method in names:
  raw=actual_functions(text,method).decode()
  functions.append({'file':name,'function':method,'sha256':hashlib.sha256(raw.encode()).hexdigest()})
  if method=='periodic_callback':
   assert raw.count('sleep(sleep_period);')==1
   raw=raw.replace('sleep(sleep_period);','controlled_sleep(sleep_period);')
  code+='\n'+raw+'\n'
code+=r'''
int main(){
 cthd_sensor a(7,"a/","A"),b(8,"b/","B");engine.named={{"A",&a},{"B",&b}};
 auto&as=*fake_files.at("a/");auto&bs=*fake_files.at("b/");
 std::string A="A",B="B",empty="";
 cthd_sensor_virtual linear(9,"linear",A,1,0);
 polling_table_entry low{0,9},hot{80000,1};linear.update_polling_table(low);linear.update_polling_table(hot);
 linear.polling.store(true);as.numbers["temp"]=85000;
 int stage=0;
 on_sleep=[&](unsigned period){
  unsigned value=123456;const bool valid=linear.read_temperature_checked(value);
  if(stage==0){require(period==1&&valid&&value==85000,"linear hot sample not published");as.read_error=-EIO;}
  if(stage==1){require(period==1,"invalid callback selected a low-temperature period");require(!valid&&value==123456&&linear.last_temp.load()==85000,"invalid callback published stale/zero cache");as.read_error=0;as.numbers["temp"]=0;}
  if(stage==2){require(period==9&&valid&&value==0,"valid zero rejected or period not updated");as.numbers["temp"]=95000;}
  if(stage==3){require(period==1&&valid&&value==95000,"linear recovery failed");linear.stop_polling.store(true);}
  ++stage;
 };
 periodic_callback(&linear);require(stage==4,"callback iteration count changed");linear.polling.store(false);
 cthd_sensor_virtual aggregate(10,"aggregate",empty,1,0);
 require(aggregate.add_target(A,.5,.5,0)==THD_SUCCESS&&aggregate.add_target(B,.5,.5,0)==THD_SUCCESS,"aggregate setup failed");
 aggregate.update_polling_table(low);aggregate.update_polling_table(hot);aggregate.polling.store(true);
 as.numbers["temp"]=95000;bs.numbers["temp"]=85000;stage=0;
 on_sleep=[&](unsigned period){
  unsigned value=123456;const bool valid=aggregate.read_temperature_checked(value);
  if(stage==0){require(period==1&&valid&&value==90000,"aggregate hot sample changed");as.numbers["temp"]=0;bs.read_error=-EIO;}
  if(stage==1){require(period==1&&!valid&&value==123456,"invalid aggregate callback altered period/cache");require(aggregate.link_sensors[0]->prev_avg==95&&aggregate.link_sensors[1]->prev_avg==85&&aggregate.last_temp.load()==90000,"failed callback partially committed filter/cache");bs.read_error=0;}
  if(stage==2){require(period==9&&valid&&value==66000,"callback recovery lost prior filter history");aggregate.stop_polling.store(true);}
  ++stage;
 };
 periodic_callback(&aggregate);require(stage==3,"aggregate callback iteration count changed");aggregate.polling.store(false);
 std::cout<<"actual periodic_callback: linear hot-invalid-zero-recovery and aggregate transaction-recovery passed\n";
}
'''
rows=[]
for tag,body in [('actual',code),('mutant-legacy-zero',code.replace('unsigned int checked_temperature;\n        const bool valid_sample = sensor->_read_temperature_checked(checked_temperature);','unsigned int checked_temperature = sensor->_read_temperature();\n        const bool valid_sample = true;'))]:
 assert tag=='actual' or body!=code
 cpp=out/(tag+'.cpp');cpp.write_text(body)
 binary=out/tag
 p=subprocess.run(['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-Wno-unused-parameter','-pthread','-I'+str(out),str(cpp),'-o',str(binary)],capture_output=True,text=True,env={**os.environ,'TMPDIR':str(out)})
 assert p.returncode==0,p.stdout+p.stderr
 p=subprocess.run([str(binary)],capture_output=True,text=True,timeout=10)
 rows.append({'variant':tag,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
 assert (p.returncode==0)==(tag=='actual'),rows
receipt={'scope':'Actual callback and checked sensor/virtual functions with deterministic sleep-only substitution; synchronous callback invocation, fake sysfs/engine. Tests seven iterations, not pthread create/join lifecycle, daemon startup or hardware. Legacy-zero callback mutant must fail the invalid-period assertion.','functions':functions,'cases':rows}
(out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))
