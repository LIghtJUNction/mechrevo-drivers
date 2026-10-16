# SPDX-License-Identifier: GPL-2.0-or-later
#!/usr/bin/env python3
"""Execute actual checked sensor/virtual functions with isolated sysfs service."""
from pathlib import Path
import hashlib,json,os,resource,subprocess,sys
import argparse
ap=argparse.ArgumentParser()
ap.add_argument('--workspace',type=Path,required=True)
ap.add_argument('--source-dir',type=Path,required=True)
ap.add_argument('--out',type=Path,required=True)
ap.add_argument('--mutation',choices=['none','cycle-guard','cache-invalidation','filter-transaction','linear-inputs'],default='none')
a=ap.parse_args();ROOT=a.workspace.resolve();SRC=a.source_dir.resolve();OUT=a.out.resolve()
sys.path.insert(0,str(ROOT/'tests'))
from test_thermald_ceiling_runtime import COMMON,SYSFS,actual_functions
OUT.mkdir(parents=True,exist_ok=True)
for name in ['thd_sensor.h','thd_sensor_virtual.h']:(OUT/name).write_bytes((SRC/name).read_bytes())
(OUT/'thd_common.h').write_text(COMMON+'\n#define thd_log_error(...) fake_log(__VA_ARGS__)\n')
sf=SYSFS.replace('    int failure = -EIO;','    int failure = -EIO;\n    int read_error = 0;\n    bool empty_read = false;').replace('        auto found = state().numbers.find(key);','        if (state().empty_read) return 0;\n        if (state().read_error) return state().read_error;\n        auto found = state().numbers.find(key);').replace('    fake_state &state() {','    void update_path(std::string p) { path=std::move(p); }\n    fake_state &state() {')
(OUT/'thd_sys_fs.h').write_text(sf)
(OUT/'thd_util.h').write_text('#pragma once\ninline bool starts_with(const std::string&a,const std::string&b){return a.rfind(b,0)==0;}\n')
code='''#include <atomic>
#include <thread>
#include <mutex>
#include <memory>
#include <vector>
#include <cmath>
#include <algorithm>
#include <cctype>
#include <iostream>
#include "thd_sensor.h"
#define private public
#include "thd_sensor_virtual.h"
#undef private
static void require(bool ok,const char*message){if(!ok){std::cerr<<message<<"\\n";std::exit(1);}}
struct cthd_engine {
  std::vector<std::unique_ptr<cthd_sensor>> sensors;
  int current_sensor_index=100;
  std::mutex thd_engine_mutex;
  int user_add_virtual_sensor(std::string,std::string,double,double);
  void send_message(int,int,void*){std::abort();}
  std::map<std::string,cthd_sensor*> named;
  cthd_sensor* search_sensor(const std::string&name){auto i=named.find(name);return i==named.end()?nullptr:i->second;}
};
static cthd_engine engine;
static cthd_engine*thd_engine=&engine;
static constexpr int PACKAGE=0,WAKEUP=0;
using gboolean=bool;using gchar=char;struct PrefObject{};struct GError{};
#define TRUE true
#define FALSE false
#define g_assert(x) require(x,"DBus object precondition failed")
std::mutex cthd_sensor_virtual::dependency_mutex;
void cthd_sensor_virtual::disable_periodic_timer(){require(!polling.load(),"test must stop cache-only mode before destruction");}
'''
selected={'thd_sensor.cpp':['cthd_sensor::cthd_sensor','cthd_sensor::read_temperature','cthd_sensor::read_temperature_checked'],
 'thd_sensor_virtual.cpp':['cthd_sensor_virtual::cthd_sensor_virtual','cthd_sensor_virtual::~cthd_sensor_virtual','cthd_sensor_virtual::add_target','cthd_sensor_virtual::dependency_reaches','cthd_sensor_virtual::sensor_update_param','cthd_sensor_virtual::update_polling_table','cthd_sensor_virtual::sensor_update','cthd_sensor_virtual::read_temperature','cthd_sensor_virtual::read_temperature_checked','cthd_sensor_virtual::_read_temperature','cthd_sensor_virtual::_read_temperature_checked'],
 'thd_engine.cpp':['cthd_engine::user_add_virtual_sensor'],
 'thd_dbus_interface.cpp':['thd_dbus_interface_add_virtual_sensor'],
}
functions=[]
for method in ['is_valid_thermal_object_name','is_valid_finite_value']:
 raw=actual_functions((SRC/'thd_util.cpp').read_text(),method)
 code+='\n'+raw.decode()+'\n'
for name,methods in selected.items():
 text=(SRC/name).read_text()
 for method in methods:
  raw=actual_functions(text,method);body=raw.decode()
  if a.mutation=='cycle-guard' and method in ['cthd_sensor_virtual::add_target','cthd_sensor_virtual::sensor_update_param']:
   old='if (dependency_reaches(sensor, this, seen)) return THD_ERROR;';assert body.count(old)==1;body=body.replace(old,'if (false && dependency_reaches(sensor, this, seen)) return THD_ERROR;')
  if a.mutation=='cache-invalidation' and method=='cthd_sensor_virtual::sensor_update_param':
   old='last_temp_valid = false;';assert body.count(old)==1;body=body.replace(old,'/* mutant: leave stale cache valid */')
  if a.mutation=='filter-transaction' and method=='cthd_sensor_virtual::_read_temperature_checked':
   old='if (link->sensor && !link->sensor->read_temperature_checked(value)) return false;';assert body.count(old)==1
   body=body.replace(old,'if (link->sensor && !link->sensor->read_temperature_checked(value)) { if (!readings.empty()) link_sensors[0]->prev_avg = (readings[0] / 1000 + link_sensors[0]->prev_avg) / 2.0; return false; }')
  if a.mutation=='linear-inputs' and method=='cthd_sensor_virtual::_read_temperature_checked':
   old='const unsigned int input_count = linear ? 1 : link_sensors.size();';assert body.count(old)==1;body=body.replace(old,'const unsigned int input_count = link_sensors.size();')
  code+='\n'+body+'\n';functions.append({'file':name,'function':method,'sha256':hashlib.sha256(raw).hexdigest()})
code+='''
int main(){
 cthd_sensor a(7,"a/","A"),b(8,"b/","B");engine.named={{"A",&a},{"B",&b}};
 auto &as=*fake_files.at("a/"),&bs=*fake_files.at("b/");
 as.numbers["temp"]=95000;bs.numbers["temp"]=85000;
 unsigned out=123456;
 require(a.read_temperature_checked(out)&&out==95000,"positive sensor read changed");
 as.numbers["temp"]=0;require(a.read_temperature_checked(out)&&out==0,"valid zero rejected");
 as.numbers["temp"]=-1;out=123456;require(!a.read_temperature_checked(out)&&out==123456,"negative read published");
 as.numbers["temp"]=95000;as.read_error=-EIO;require(!a.read_temperature_checked(out)&&out==123456,"IO error published");
 as.read_error=0;as.empty_read=true;require(!a.read_temperature_checked(out)&&out==123456,"EOF published");as.empty_read=false;
 a.set_scale(0);require(!a.read_temperature_checked(out)&&out==123456,"zero scale accepted");a.set_scale(1);
 std::string A="A",B="B",empty="",constant="constant";
 cthd_sensor_virtual v(9,"V",A,1,0);
 require(v.read_temperature_checked(out)&&out==95000,"single linked positive changed");
 as.numbers["temp"]=0;require(v.read_temperature_checked(out)&&out==0,"virtual valid zero rejected");
 as.read_error=-EIO;out=123456;require(!v.read_temperature_checked(out)&&out==123456,"virtual invalid linked source published");
 v.polling.store(true);require(!v.read_temperature_checked(out)&&out==123456,"invalid virtual cache published");v.polling.store(false);
 as.read_error=0;as.numbers["temp"]=95000;
 cthd_sensor_virtual multi(10,"multi",empty,1,0);multi.add_target(A,.5,.5,0);multi.add_target(B,.5,.5,0);
 require(multi.read_temperature_checked(out)&&out==90000,"virtual aggregate changed");
 const double prior=multi.link_sensors[0]->prev_avg;
 as.numbers["temp"]=0;bs.read_error=-EIO;out=123456;
 require(!multi.read_temperature_checked(out)&&out==123456,"partially invalid aggregate published");
 require(multi.link_sensors[0]->prev_avg==prior,"failed aggregate partially changed filter history");
 require(!multi.last_temp_valid && multi.last_temp.load()==90000,"invalid aggregate cache mutated value or stayed valid");
 bs.read_error=0;require(multi.read_temperature_checked(out)&&out==66000,"valid recovery did not preserve prior filter history");
 cthd_sensor_virtual constants(11,"constants",empty,1,0);constants.add_target(constant,10,0,0);
 require(constants.read_temperature_checked(out)&&out==10000,"intentional constant term rejected");
 cthd_sensor_virtual noinputs(12,"no-inputs",empty,1,0);out=123456;
 require(!noinputs.read_temperature_checked(out)&&out==123456,"empty virtual source fabricated valid zero");
 cthd_sensor_virtual negative_formula(13,"negative-formula",A,1,-100000);
 require(negative_formula.read_temperature_checked(out)&&out==0,"valid negative formula clamp semantics changed");
 // An ignored secondary source must stay ignored by the original linear algorithm.
 bs.read_error=-EIO;require(v.sensor_update_param(B,1,0)==THD_SUCCESS,"valid named update rejected");
 as.numbers["temp"]=95000;require(v.read_temperature_checked(out)&&out==95000,"linear path consumed originally ignored second dependency");bs.read_error=0;
 std::unique_ptr<cthd_sensor_virtual> dv_owner(new cthd_sensor_virtual(20,"DBV",A,1,0));
 auto *dv=dv_owner.get();engine.named["DBV"]=dv;engine.sensors.push_back(std::move(dv_owner));
 require(dv->read_temperature_checked(out),"DBus update cache setup failed");
 PrefObject object;char dbv[]="DBV",missing[]="missing",sourceB[]="B";
 const auto old_count=dv->link_sensors.size();const auto old_temp=dv->last_temp.load();
 require(!thd_dbus_interface_add_virtual_sensor(&object,dbv,dbv,2,1,nullptr),"actual DBus self update accepted");
 require(dv->link_sensors.size()==old_count&&dv->last_temp_valid&&dv->last_temp.load()==old_temp&&dv->multiplier==1&&dv->offset==0,"failed self update changed vector/cache/parameters");
 require(!thd_dbus_interface_add_virtual_sensor(&object,dbv,missing,2,1,nullptr),"missing DBus dependency accepted");
 require(dv->link_sensors.size()==old_count&&dv->last_temp_valid,"missing update changed cached state");
 require(thd_dbus_interface_add_virtual_sensor(&object,dbv,sourceB,.5,0,nullptr),"actual DBus successful update rejected");
 require(!dv->last_temp_valid&&dv->link_sensors.size()==old_count+1,"successful update did not invalidate cache/append source");
 require(dv->read_temperature_checked(out)&&out==47500,"successful update changed original first-source formula");
 cthd_sensor_virtual cycleA(21,"cycleA",empty,1,0),cycleB(22,"cycleB",empty,1,0);
 std::string ca="cycleA",cb="cycleB";engine.named[ca]=&cycleA;engine.named[cb]=&cycleB;
 require(cycleA.add_target(A,1,0,0)==THD_SUCCESS&&cycleA.read_temperature_checked(out),"cycle fixture setup failed");
 require(cycleB.add_target(ca,1,0,0)==THD_SUCCESS,"acyclic virtual graph rejected");
 const auto cycle_count=cycleA.link_sensors.size();const auto cycle_temp=cycleA.last_temp.load();
 require(cycleA.add_target(cb,1,0,0)==THD_ERROR&&cycleA.sensor_update_param(cb,1,0)==THD_ERROR,"transitive cycle update accepted");
 require(cycleA.link_sensors.size()==cycle_count&&cycleA.last_temp_valid&&cycleA.last_temp.load()==cycle_temp,"cycle rejection changed graph/cache");
 as.numbers["temp"]=95000;require(v.read_temperature_checked(out),"cache setup failed");v.polling.store(true);
 require(v.sensor_update_param(A,1,0)==THD_SUCCESS,"polling cache update rejected");out=123456;
 require(!v.read_temperature_checked(out)&&out==123456,"parameter update published stale valid cache");
 require(v._read_temperature_checked(out),"post-update cache refill failed");
 std::atomic<int> bad(0),ready(0);
 auto await_all=[&]{++ready;while(ready.load()!=3)std::this_thread::yield();};
 std::thread writer([&]{await_all();for(int i=0;i<10000;++i){as.read_error=i%3==0?-EIO:0;as.numbers["temp"]=i%2?0:95000;unsigned value=123456;
   const bool ok=v._read_temperature_checked(value);if((as.read_error&&ok)||(!as.read_error&&!ok)||(!ok&&value!=123456))++bad;}});
 std::thread reader([&]{await_all();for(int i=0;i<10000;++i){unsigned value=123456;const bool ok=v.read_temperature_checked(value);if((ok&&value!=0&&value!=95000)||(!ok&&value!=123456))++bad;}});
 std::thread parameter_writer([&]{await_all();for(int i=0;i<100;++i)if(v.sensor_update_param(A,1,0)!=THD_SUCCESS)++bad;});
 writer.join();reader.join();parameter_writer.join();v.polling.store(false);require(bad.load()==0,"cache value/validity snapshots were inconsistent");
 // Two aggregate graph writers must serialize the cycle check and mutation.
 cthd_sensor_virtual c(30,"C",empty,1,0),d(31,"D",empty,1,0);
 std::string C="C",D="D";engine.named[C]=&c;engine.named[D]=&d;
 require(c.add_target(constant,1,0,0)==THD_SUCCESS&&d.add_target(constant,1,0,0)==THD_SUCCESS,"concurrent graph setup failed");
 std::atomic<bool> go(false);int cr=99,dr=99;
 std::thread cg([&]{while(!go.load())std::this_thread::yield();cr=c.add_target(D,1,0,0);});
 std::thread dg([&]{while(!go.load())std::this_thread::yield();dr=d.add_target(C,1,0,0);});go.store(true);cg.join();dg.join();
 require((cr==THD_SUCCESS&&dr==THD_ERROR)||(dr==THD_SUCCESS&&cr==THD_ERROR),"concurrent graph writers accepted a dependency cycle");
 require(c.read_temperature_checked(out)&&d.read_temperature_checked(out),"accepted acyclic concurrent graph cannot be read");
 std::cout<<"checked-base, zero, negative, IO, EOF, scale, virtual-single, invalid-cache, aggregate-transaction, recovery, constants, no-inputs, clamp, actual-DBus-update+rollback, direct/transitive-cycle-rejection, polling-update-invalidation, concurrent-cache+parameter-writer: passed\\n";
}
'''
(OUT/'virtual.cpp').write_text(code)
argv=['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-Wno-unused-parameter','-pthread','-I'+str(OUT),str(OUT/'virtual.cpp'),'-o',str(OUT/'virtual')]
p=subprocess.run(argv,capture_output=True,text=True,timeout=60,env={**os.environ,'TMPDIR':str(OUT)})
if p.returncode:raise RuntimeError(p.stdout+p.stderr)
p=subprocess.run([str(OUT/'virtual')],capture_output=True,text=True,timeout=10,preexec_fn=lambda:resource.setrlimit(resource.RLIMIT_CORE,(0,0)))
receipt={'mutation':a.mutation,'scope':'Actual selected checked API/virtual functions and actual class declarations; fake sysfs/engine, test-only visibility of private fields. Manual cache publisher/reader/parameter-writer threads and actual DBus-to-engine update functions with service doubles, not real DBus IPC or the periodic timer lifecycle or full daemon.','functions':functions,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr}
(OUT/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(p.stdout,p.stderr);raise SystemExit(p.returncode)
