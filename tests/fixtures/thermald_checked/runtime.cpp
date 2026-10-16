// SPDX-License-Identifier: GPL-2.0-or-later
// Exact extracted sensor/zone functions; mock sysfs, engine and trip callback only.
#include <cassert>
#include <tuple>
#include <iostream>
#include "thd_sensor.h"
enum sensor_relate_t { SENSOR_INDEPENDENT, SENSORS_CORELATED };
struct fake_engine {int get_preference(){return 0;}};
static fake_engine engine;
static fake_engine*thd_engine=&engine;
class cthd_zone {
public:
 bool zone_active=true;
 unsigned zone_temp=12345;
 int index=0;
 sensor_relate_t sensor_rel=SENSOR_INDEPENDENT;
 std::vector<cthd_sensor*> sensors;
 std::vector<std::tuple<int,unsigned,unsigned>> calls;
 void thermal_zone_temp_change(int id,unsigned temp,int){calls.emplace_back(id,temp,zone_temp);}
 void read_zone_temp();
 void update_zone_preference();
};
/* @ACTUAL_FUNCTIONS@ */
int main(int argc,char**argv){
 assert(argc==2);std::string name=argv[1];
 cthd_sensor a(7,"a/","A"),b(8,"b/","B");
 auto &as=*fake_files.at("a/"),&bs=*fake_files.at("b/");
 as.numbers["temp"]=85000;bs.numbers["temp"]=73000;
 cthd_zone zone;zone.sensors={&a};
 if(name=="io"||name=="eof"||name=="negative"||name=="second"||name=="pref-invalid"){
  if(name=="io")as.read_error=-EIO;
  if(name=="eof")as.empty_read=true;
  if(name=="negative")as.numbers["temp"]=-1;
  if(name=="second"||name=="pref-invalid"){zone.sensors.push_back(&b);bs.read_error=-EIO;}
  if(name=="pref-invalid")zone.update_zone_preference();else zone.read_zone_temp();
  assert(zone.calls.empty()&&zone.zone_temp==12345);return 0;
 }
 if(name=="preference"||name=="preference-zero"){
  if(name=="preference-zero")as.numbers["temp"]=0;else zone.sensors.push_back(&b);
  zone.update_zone_preference();const int count=zone.sensors.size();assert(zone.calls.size()==unsigned(count*2));
  for(int i=0;i<count;++i){unsigned t=i?73000:unsigned(as.numbers["temp"]);
   assert(zone.calls[i]==std::make_tuple(7+i,0u,t));
   assert(zone.calls[count+i]==std::make_tuple(7+i,t,t));}
  assert(zone.zone_temp==unsigned(count==2?73000:as.numbers["temp"]));return 0;
 }
 if(name=="zero"||name=="normal"){
  as.numbers["temp"]=name=="zero"?0:85000;zone.read_zone_temp();
  assert(zone.calls.size()==1&&std::get<1>(zone.calls[0])==unsigned(as.numbers["temp"]));return 0;
 }
 if(name=="correlated-zero"){
  as.numbers["temp"]=0;zone.sensor_rel=SENSORS_CORELATED;zone.read_zone_temp();assert(zone.calls.empty()&&zone.zone_temp==0);return 0;
 }
#ifdef CHECKED_API
 if(name=="checked-api"){
  unsigned out=111111;assert(a.read_temperature_checked(out)&&out==85000);
  as.numbers["temp"]=0;assert(a.read_temperature_checked(out)&&out==0);
  for(int e:{-EIO,-ENODEV,-ETIMEDOUT}){as.read_error=e;out=111111;assert(!a.read_temperature_checked(out)&&out==111111);}
  as.read_error=0;as.empty_read=true;assert(!a.read_temperature_checked(out)&&out==111111);as.empty_read=false;
  as.numbers["temp"]=-1;assert(!a.read_temperature_checked(out)&&out==111111);
  as.numbers["temp"]=85000;for(int scale:{0,-1}){a.set_scale(scale);assert(!a.read_temperature_checked(out)&&out==111111);}
  a.set_scale(1000);assert(a.read_temperature_checked(out)&&out==85);return 0;
 }
#endif
 return 2;
}
