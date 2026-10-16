// Fixture service/storage declarations; production bodies inserted verbatim.
#include <iostream>
#include <ctime>
#include <fcntl.h>
#include <unistd.h>
#include <inttypes.h>
#include "thd_sensor.h"
static constexpr int EXPECTED_SENSOR_READ_ERROR=-3;
static void require(bool ok,const char*m){if(!ok){std::cerr<<m<<"\n";std::exit(1);}}
static time_t fixture_time=1000;
extern "C" time_t time(time_t*p) noexcept {if(p)*p=fixture_time;return fixture_time;}

/* @CONDITION_DECLARATIONS@ */
#define OEM_CONDITION_BASE_ID 0x1000
#define SW_OEM_CONDITION_BASE_ID 0x2000
static const char*condition_names[100]={};
static const char*comp_strs[5]={};
class cthd_engine {
public:
 std::vector<std::unique_ptr<cthd_sensor>> sensors;
 int base_starts=0;
 bool debug_mode_on(){return false;}
 cthd_sensor*search_sensor(const std::string&name){for(auto&s:sensors)if(s->get_sensor_type()==name)return s.get();return nullptr;}
 cthd_sensor*user_get_sensor(int i){return i>=0&&i<(int)sensors.size()?sensors[i].get():nullptr;}
 int get_sensor_temperature(int,unsigned int*);
 int thd_engine_start(){++base_starts;return THD_SUCCESS;}
};
static cthd_engine base_engine;
static cthd_engine*thd_engine=&base_engine;
struct fake_object{};
struct libevdev{int fd;};
static int dynamic_frees=0,object_frees=0;
static int libevdev_get_fd(libevdev*p){return p->fd;}
static void libevdev_free(libevdev*p){++dynamic_frees;delete p;}
template<class T>static void g_clear_object(T**p){if(*p){++object_frees;delete *p;*p=nullptr;}}
struct adaptive_target {uint64_t target_id;};
struct psvt {};
class cthd_gddv {
public:
 std::vector<std::vector<condition>>conditions;
 std::vector<adaptive_target>targets;
 int current_target_matched=7,verify_result=1,aggressive_calls=0,aggressive_result=0;
 fake_object*upower_client=nullptr,*power_profiles_daemon=nullptr;
 libevdev*tablet_dev=nullptr,*lid_dev=nullptr;
 void destroy_dynamic_sources();
 int compare_condition(const condition&,int);
 int compare_time(const condition&);
 int evaluate_temperature_condition(condition);
 int evaluate_condition(condition&);
 int evaluate_condition_set(std::vector<condition>&);
 int evaluate_conditions();
 int verify_conditions(){return verify_result;}
 int find_agressive_target(){++aggressive_calls;return aggressive_result;}
 psvt*find_def_psvt(){std::abort();}
 void gddv_free();
 int evaluate_os_type_condition(const condition&){std::abort();}
 int evaluate_oem_condition(const condition&){std::abort();}
 int evaluate_lid_condition(const condition&){std::abort();}
 int evaluate_ac_condition(const condition&){std::abort();}
 int evaluate_workload_condition(const condition&){std::abort();}
 int evaluate_platform_type_condition(const condition&){std::abort();}
 int evaluate_power_slider_condition(const condition&){std::abort();}
};
static constexpr int EXCLUSIVE=0;
class cthd_engine_adaptive:public cthd_engine {
public:
 cthd_gddv gddv;
 bool passive_def_only=false;
 int fallback_id=-1,current_matched_target=7,policy_active=1,int3400_installed=1;
 int defaults=0,executed=0,control_modes=0;
 void update_engine_state();
 int thd_engine_start();
 void exec_fallback_target(uint64_t){++executed;}
 void execute_target(adaptive_target&){++executed;}
 void install_passive_default(){++defaults;}
 void set_control_mode(int){++control_modes;}
};

/* @ACTUAL_FUNCTIONS@ */
static condition temperature_condition(int target=9,int argument=2732){
 condition c{};c.condition=Temperature;c.device="scope.A";c.comparison=ADAPTIVE_EQUAL;c.argument=argument;c.target=target;c.state_entry_time=456;return c;
}
static condition motion_condition(int argument,int target=8){condition c{};c.condition=Motion;c.argument=argument;c.target=target;c.state_entry_time=123;return c;}
static condition default_condition(int target=10){condition c{};c.condition=Default;c.target=target;return c;}
static std::vector<std::vector<condition>> invalid_conditions(){return {{motion_condition(1)},{temperature_condition()},{default_condition()}};}
static void invalid_evaluation(){
 auto&state=*fake_files.at("a/");state.read_error=-EIO;
 unsigned out=123456;require(base_engine.get_sensor_temperature(0,&out)==THD_ERROR&&out==123456,"engine must preserve output on invalid sensor read");
 cthd_gddv c;c.conditions=invalid_conditions();
 require(c.evaluate_conditions()==EXPECTED_SENSOR_READ_ERROR,"evaluated invalid temperature must return sensor sentinel");
 require(c.current_target_matched==7,"invalid evaluation changed matched target");
 require(c.conditions[0][0].state_entry_time==123&&c.conditions[1][0].state_entry_time==456,"invalid evaluation failed to roll back all condition times");
 cthd_engine_adaptive adaptive;adaptive.gddv.conditions=invalid_conditions();adaptive.update_engine_state();
 require(adaptive.current_matched_target==7&&adaptive.defaults==0&&adaptive.executed==0&&adaptive.int3400_installed==1&&adaptive.policy_active==1,"adaptive invalid runtime changed policy or installed default");
}
static void short_circuit(){
 auto&state=*fake_files.at("a/");state.read_error=-EIO;const auto reads=state.read_attempts;
 cthd_gddv c;c.conditions={{motion_condition(1),temperature_condition()},{default_condition()}};
 require(c.evaluate_conditions()==10&&state.read_attempts==reads,"false leading predicate must skip invalid temperature and match later default");
 c.conditions={{default_condition(12)},{temperature_condition()}};
 require(c.evaluate_conditions()==12&&state.read_attempts==reads,"earlier matched set must skip later invalid temperature");
 auto ignored=temperature_condition();ignored.ignore_condition=1;c.conditions={{ignored},{default_condition()}};
 require(c.evaluate_conditions()==10&&state.read_attempts==reads,"ignored temperature must remain ordinary nonmatch without reading");
 auto missing=temperature_condition();missing.device="missing";c.conditions={{missing},{default_condition()}};
 require(c.evaluate_conditions()==10&&state.read_attempts==reads,"missing named sensor preserves existing ordinary nonmatch semantics");
 auto timed=motion_condition(0);timed.time=5;timed.time_comparison=ADAPTIVE_GREATER_OR_EQUAL;timed.state_entry_time=0;
 c.conditions={{timed,temperature_condition()}};
 require(c.evaluate_conditions()==-1&&state.read_attempts==reads&&c.conditions[0][0].state_entry_time==fixture_time,"unexpired actual time predicate must skip invalid temperature");
 c.conditions={{motion_condition(0),temperature_condition()},{default_condition()}};
 require(c.evaluate_conditions()==EXPECTED_SENSOR_READ_ERROR&&state.read_attempts==reads+1,"applicable temperature predicate must propagate invalid and block later default");
}
static void startup_invalid(const char*dir,int verify){
 auto&state=*fake_files.at("a/");state.read_error=-EIO;
 cthd_engine_adaptive startup;startup.gddv.verify_result=verify;startup.gddv.conditions=invalid_conditions();
 const std::string file1=std::string(dir)+"/tablet",file2=std::string(dir)+"/lid";
 int fd1=::open(file1.c_str(),O_CREAT|O_RDWR|O_TRUNC,0600);require(fd1>=0,"fixture tablet regular FD open failed");
 int fd2=verify ? fd1 : ::open(file2.c_str(),O_CREAT|O_RDWR|O_TRUNC,0600);require(fd2>=0,"fixture lid regular FD open failed");
 startup.gddv.tablet_dev=new libevdev{fd1};startup.gddv.lid_dev=verify?startup.gddv.tablet_dev:new libevdev{fd2};
 startup.gddv.upower_client=new fake_object;startup.gddv.power_profiles_daemon=new fake_object;
 const int before_frees=dynamic_frees,before_objects=object_frees;
 const int result=startup.thd_engine_start();
 require(startup.defaults==0&&startup.executed==0&&startup.gddv.aggressive_calls==0&&startup.base_starts==0,"invalid startup must not enter aggressive/default/base startup fallback");
 require(result==THD_ERROR,"invalid startup must return failure");
 require(::fcntl(fd1,F_GETFD)==-1&&errno==EBADF&&::fcntl(fd2,F_GETFD)==-1&&errno==EBADF,"invalid startup must actually close owned regular FDs");
 require(startup.gddv.tablet_dev==nullptr&&startup.gddv.lid_dev==nullptr&&startup.gddv.upower_client==nullptr&&startup.gddv.power_profiles_daemon==nullptr,"invalid startup retained dynamic source ownership");
 require(dynamic_frees-before_frees==(verify?1:2)&&object_frees-before_objects==2,"invalid startup freed aliased/nonaliased sources incorrectly");
 startup.gddv.gddv_free();require(dynamic_frees-before_frees==(verify?1:2)&&object_frees-before_objects==2,"repeated cleanup double freed source objects");
}
static void valid_and_no_match(){
 auto&state=*fake_files.at("a/");state.read_error=0;state.numbers["temp"]=0;
 unsigned out=123456;require(base_engine.get_sensor_temperature(0,&out)==THD_SUCCESS&&out==0,"engine must accept valid zero");
 cthd_gddv c;c.conditions={{temperature_condition()}};require(c.evaluate_conditions()==9,"valid zero must match decikelvin 2732");
 state.numbers["temp"]=95000;c.conditions={{temperature_condition(11,3682)}};require(c.evaluate_conditions()==11,"positive temperature conversion changed");
 state.numbers["temp"]=85000;c.conditions={{temperature_condition(11,3682)}};require(c.evaluate_conditions()==-1,"valid temperature mismatch must remain ordinary no-match");
 cthd_engine_adaptive runtime;runtime.gddv.conditions=c.conditions;runtime.update_engine_state();
 require(runtime.current_matched_target==-1&&runtime.defaults==1,"ordinary runtime no-match must retain default behavior");
 for(int verify:{0,1}){
  cthd_engine_adaptive startup;startup.gddv.verify_result=verify;startup.gddv.conditions=c.conditions;
  require(startup.thd_engine_start()==THD_SUCCESS&&startup.base_starts==1&&startup.defaults==1&&startup.gddv.aggressive_calls==verify,"ordinary startup no-match must retain existing default/aggressive paths");
 }
 cthd_engine_adaptive passive;passive.passive_def_only=true;state.read_error=-EIO;const auto reads=state.read_attempts;
 require(passive.thd_engine_start()==THD_SUCCESS&&passive.defaults==1&&passive.base_starts==1&&state.read_attempts==reads,"explicit passive-only startup must not invent a temperature read");
}
int main(int argc,char**argv){
 require(argc==3,"case and fixture output directory required");
 base_engine.sensors.emplace_back(new cthd_sensor(7,"a/","A"));fake_files.at("a/")->numbers["temp"]=95000;
 const std::string name=argv[1];
 if(name=="invalid")invalid_evaluation();
 else if(name=="short-circuit")short_circuit();
 else if(name=="startup-unsupported")startup_invalid(argv[2],1);
 else if(name=="startup-supported")startup_invalid(argv[2],0);
 else if(name=="valid-no-match")valid_and_no_match();
 else require(false,"unknown fixture case");
 std::cout<<name<<": passed\n";
}
