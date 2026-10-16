# SPDX-License-Identifier: GPL-2.0-or-later
#!/usr/bin/env python3
"""Actual thermald periodic callback regression; explicit paths, no hardware/daemon."""
import argparse,hashlib,json,os,resource,subprocess,sys
from pathlib import Path
ap=argparse.ArgumentParser()
ap.add_argument('--workspace',type=Path,required=True)
ap.add_argument('--source-dir',type=Path,required=True)
ap.add_argument('--out',type=Path,required=True)
ap.add_argument('--expect-restart-invalidation',action='store_true')
ap.add_argument('--prior-cold',type=int,choices=[0,75000],default=0)
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
#include <condition_variable>
#include <chrono>
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
struct fake_engine {int get_preference(){return 0;}std::map<std::string,cthd_sensor*> named;cthd_sensor* search_sensor(const std::string&n){auto i=named.find(n);return i==named.end()?nullptr:i->second;}};
static fake_engine engine;static fake_engine*thd_engine=&engine;
std::mutex cthd_sensor_virtual::dependency_mutex;

static std::mutex gate;static std::condition_variable cv;
static unsigned sleepers=0,tokens=0;static cthd_sensor_virtual*active=nullptr;
static std::atomic<int> creates(0),joins(0),live(0),attr_inits(0),attr_destroys(0);
static bool fail_create=false;static bool hold_start=false;
struct launch {void*(*fn)(void*);void*arg;};
static void*trampoline(void*p){std::unique_ptr<launch>x(static_cast<launch*>(p));++live;{std::unique_lock<std::mutex> l(gate);require(cv.wait_for(l,std::chrono::seconds(2),[]{return !hold_start;}),"worker launch gate timeout");}void*r=x->fn(x->arg);--live;return r;}
static int counted_create(pthread_t*t,const pthread_attr_t*a,void*(*f)(void*),void*p){
 if(fail_create){fail_create=false;return EAGAIN;}
 std::unique_ptr<launch>x(new launch{f,p});int ret=::pthread_create(t,a,trampoline,x.get());if(!ret){x.release();++creates;}return ret;
}
static int counted_join(pthread_t t,void**r){int ret=::pthread_join(t,r);if(!ret)++joins;return ret;}
static int counted_attr_init(pthread_attr_t*p){int r=::pthread_attr_init(p);if(!r)++attr_inits;return r;}
static int counted_attr_destroy(pthread_attr_t*p){int r=::pthread_attr_destroy(p);if(!r)++attr_destroys;return r;}
static unsigned controlled_sleep(unsigned){
 std::unique_lock<std::mutex>l(gate);++sleepers;cv.notify_all();
 auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(2);
 while(!tokens&&!active->stop_polling.load()){
  require(std::chrono::steady_clock::now()<deadline,"controlled sleep timed out waiting for test step");
  cv.wait_for(l,std::chrono::milliseconds(2));
 }
 if(tokens) --tokens;
 return 0;
}
static void await_sleep(unsigned n){std::unique_lock<std::mutex>l(gate);require(cv.wait_for(l,std::chrono::seconds(2),[&]{return sleepers>=n;}),"worker did not reach controlled sleep");}
static void resume(){std::lock_guard<std::mutex>l(gate);++tokens;cv.notify_all();}

'''
methods={'thd_sensor.cpp':['cthd_sensor::cthd_sensor','cthd_sensor::read_temperature','cthd_sensor::read_temperature_checked'], 'thd_sensor_virtual.cpp':['cthd_sensor_virtual::cthd_sensor_virtual','cthd_sensor_virtual::~cthd_sensor_virtual','cthd_sensor_virtual::add_target','cthd_sensor_virtual::dependency_reaches','cthd_sensor_virtual::update_polling_table','cthd_sensor_virtual::read_temperature','cthd_sensor_virtual::read_temperature_checked','cthd_sensor_virtual::_read_temperature','cthd_sensor_virtual::_read_temperature_checked','periodic_callback','cthd_sensor_virtual::enable_periodic_timer','cthd_sensor_virtual::disable_periodic_timer']}
functions=[]
for name,names in methods.items():
 text=(a.source_dir/name).read_text()
 for method in names:
  raw=actual_functions(text,method).decode()
  functions.append({'file':name,'function':method,'sha256':hashlib.sha256(raw.encode()).hexdigest()})
  if method=='periodic_callback':
   assert raw.count('sleep(sleep_period);')==1
   raw=raw.replace('sleep(sleep_period);','controlled_sleep(sleep_period);')

  if method in ['cthd_sensor_virtual::enable_periodic_timer','cthd_sensor_virtual::disable_periodic_timer']:
   for call in ['pthread_create','pthread_join','pthread_attr_init','pthread_attr_destroy']:
    raw=raw.replace(call+'(', 'counted_'+call.removeprefix('pthread_')+'(')
  code+='\n'+raw+'\n'
# Actual inline zone activation and actual checked zone read; notification is recorded,
# not substituted with a guessed hardware/controller outcome.
zh=(a.source_dir/'thd_zone.h').read_text()
code+='\nenum sensor_relate_t { SENSOR_INDEPENDENT, SENSORS_CORELATED };\nclass cthd_zone { public: int index=0;std::vector<cthd_sensor*> sensors; bool zone_active=false; unsigned zone_temp=12345; sensor_relate_t sensor_rel=SENSOR_INDEPENDENT; unsigned notifications=0; unsigned last_notification=12345; void read_zone_temp(); void thermal_zone_temp_change(int,unsigned temp,int){++notifications;last_notification=temp;}\n'
for method in ['set_zone_active','set_zone_inactive']:
 raw=actual_functions(zh,method).decode();code+=raw+'\n';functions.append({'file':'thd_zone.h','function':method,'sha256':hashlib.sha256(raw.encode()).hexdigest()})
code+='};\n'
raw=actual_functions((a.source_dir/'thd_zone.cpp').read_text(),'cthd_zone::read_zone_temp').decode();code+=raw+'\n';functions.append({'file':'thd_zone.cpp','function':'cthd_zone::read_zone_temp','sha256':hashlib.sha256(raw.encode()).hexdigest()})
code+=r'''
int main(){
 constexpr unsigned PRIOR_COLD=@PRIOR_COLD@;
 cthd_sensor a(7,"a/","A");engine.named={{"A",&a}};auto&as=*fake_files.at("a/");
 std::string A="A";unsigned out=123456;
 {
 cthd_sensor_virtual sensor(9,"linear",A,1,0);active=&sensor;cthd_zone zone;zone.sensors.push_back(&sensor);
 polling_table_entry low{0,9},hot{80000,1};sensor.update_polling_table(low);sensor.update_polling_table(hot);
 as.numbers["temp"]=85000;fail_create=true;zone.set_zone_active();
 require(creates==0&&!sensor.polling.load()&&!sensor.poll_thread_started&&attr_inits==1&&attr_destroys==1,"create failure leaked attributes or published running state");
 require(sensor.read_temperature_checked(out)&&out==85000,"create failure disabled synchronous sensor reads");
 zone.set_zone_inactive();require(joins==0,"failed create attempted invalid join");
 zone.set_zone_active();await_sleep(1);
 require(creates==1&&live==1&&sensor.read_temperature_checked(out)&&out==85000,"successful start failed to publish real worker sample");
 zone.set_zone_active();require(creates==1&&sensor.read_temperature_checked(out)&&out==85000,"repeat enable created duplicate worker or invalidated active cache");
 as.read_error=-EIO;resume();await_sleep(2);out=123456;
 require(!sensor.read_temperature_checked(out)&&out==123456,"real worker published invalid input");
 zone.set_zone_inactive();require(joins==1&&live==0&&!sensor.polling.load()&&!sensor.poll_thread_started,"disable failed to join worker");
 zone.set_zone_inactive();require(joins==1,"repeat disable attempted duplicate join");
 // New-cache counterexample: invalidated cache cannot publish an old success.
 {std::lock_guard<std::mutex> l(gate);hold_start=true;}
 zone.set_zone_active();out=123456;
 require(!sensor.read_temperature_checked(out)&&out==123456,"invalid cache became valid merely by enabling");
 {std::lock_guard<std::mutex> l(gate);hold_start=false;cv.notify_all();}
 await_sleep(3);zone.set_zone_inactive();
 for(int i=0;i<3;++i){
  as.read_error=0;as.numbers["temp"]=i?95000:PRIOR_COLD;
  if(i==1){as.read_error=-EIO;std::lock_guard<std::mutex> l(gate);hold_start=true;}
  zone.set_zone_active();
  if(i==1){
   out=123456;require(sensor.read_temperature_checked(out)&&out==PRIOR_COLD,"expected preserved cold cache before first restart sample was not observable");
   auto before=zone.notifications;zone.read_zone_temp();require(zone.notifications==before+1&&zone.last_notification==PRIOR_COLD,"actual reactivated zone did not submit stale cold cache");
   {std::lock_guard<std::mutex> l(gate);hold_start=false;cv.notify_all();}
   await_sleep(4+i);out=123456;require(!sensor.read_temperature_checked(out)&&out==123456,"first failed restart sample did not invalidate old cache");
   before=zone.notifications;zone.read_zone_temp();require(zone.notifications==before,"failed first restart sample still notified cooling");
   zone.set_zone_inactive();as.read_error=0;zone.set_zone_active();await_sleep(5+i);
  }else await_sleep(4+i+(i==2?1:0));
  out=123456;require(sensor.read_temperature_checked(out)&&out==static_cast<unsigned>(i?95000:PRIOR_COLD),"restart did not publish valid zero/positive fresh worker sample");
  if(i<2){zone.set_zone_inactive();require(live==0&&joins==creates,"restart disable leaked live worker");}
 }
 // Last worker intentionally remains active: actual destructor must stop/join it.
 }
 require(live==0&&creates==joins&&attr_inits==attr_destroys,"destructor leaked worker or pthread attributes");
 std::cout<<"real pthread lifecycle passed: create failure/retry, repeated enable/disable, invalid worker cache, zero/positive restarts, active destructor; creates="<<creates<<" joins="<<joins<<" attr_init="<<attr_inits<<" attr_destroy="<<attr_destroys<<"\n";
}
'''

code=code.replace('@PRIOR_COLD@',str(a.prior_cold))
if a.expect_restart_invalidation:
 old='out=123456;require(sensor.read_temperature_checked(out)&&out==PRIOR_COLD,"expected preserved cold cache before first restart sample was not observable");'
 new='out=123456;require(!sensor.read_temperature_checked(out)&&out==123456,"restart accepted stale cold cache before first worker sample");'
 assert code.count(old)==1;code=code.replace(old,new)
 old='require(zone.notifications==before+1&&zone.last_notification==PRIOR_COLD,"actual reactivated zone did not submit stale cold cache");'
 new='require(zone.notifications==before,"actual reactivated zone submitted stale cold cache");'
 assert code.count(old)==1;code=code.replace(old,new)

rows=[]
for tag,body in [('actual-lifecycle',code)]:
 assert tag=='actual-lifecycle'
 cpp=out/(tag+'.cpp');cpp.write_text(body)
 binary=out/tag
 p=subprocess.run(['c++','-std=c++17','-O2','-Wall','-Wextra','-Werror','-Wno-unused-parameter','-pthread','-I'+str(out),str(cpp),'-o',str(binary)],capture_output=True,text=True,env={**os.environ,'TMPDIR':str(out)})
 assert p.returncode==0,p.stdout+p.stderr
 p=subprocess.run([str(binary)],capture_output=True,text=True,timeout=10)
 rows.append({'variant':tag,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
 assert p.returncode==0,rows
receipt={'scope':'Actual enable/disable/destructor/callback with genuine pthread create/join/attr primitives wrapped only for counters and one injected EAGAIN. Sleep is a bounded interruptible condition-variable fixture, not production sleep latency. Synchronous sensor service doubles; no thermald process/hardware.','prior_cold':a.prior_cold,'expect_restart_invalidation':a.expect_restart_invalidation,'functions':functions,'cases':rows}
(out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))
