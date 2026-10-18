/* Actual provider/engine/callers; only transport and kernel runtime are mocked. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
/* Kernel sched.h reserves this identifier. Bare current must fail here too. */
#define current get_current()
/* @ACTUAL_HEADERS@ */
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]))
#define EXPORT_SYMBOL(x)
#define pr_debug(...) do {} while (0)
#define pr_err(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define TUXEDO_ERROR(...) do {} while (0)
#define lockdep_assert_held(x) do {} while (0)
#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a,b,c) (((a)<<16)|((b)<<8)|(c))
#define THIS_MODULE NULL
struct uniwill_device_features_t { u8 model; bool uniwill_has_universal_ec_fan_control; bool uniwill_custom_profile_mode_needed; };
static struct uniwill_device_features_t uniwill_device_features;
static struct uniwill_device_features_t *uniwill_get_device_features(void) { return &uniwill_device_features; }
enum { DMI_SYS_VENDOR, DMI_PRODUCT_NAME, DMI_BIOS_VERSION };
static const char *dmi_values[3];
static bool dmi_match(int field, const char *value) { return strcmp(dmi_values[field], value) == 0; }
static int module_references;
static bool module_get_allowed = true;
static bool try_module_get(void *module) { (void)module; if (!module_get_allowed) return false; ++module_references; return true; }
static void module_put(void *module) { (void)module; assert(module_references > 0); --module_references; }
struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER }
static void mutex_lock(struct mutex *m) { assert(pthread_mutex_lock(&m->native) == 0); }
static void mutex_unlock(struct mutex *m) { assert(pthread_mutex_unlock(&m->native) == 0); }
struct work_struct { int unused; };
struct delayed_work { struct work_struct work; bool pending, running; };
#define DECLARE_DELAYED_WORK(name, handler) struct delayed_work name
static int work_queues, work_cancels;
static bool schedule_delayed_work(struct delayed_work *w, unsigned int delay) { (void)delay; ++work_queues; bool old = w->pending; w->pending = true; return !old; }
static bool delayed_work_pending(struct delayed_work *w) { return w->pending; }
static void cancel_delayed_work_sync(struct delayed_work *w) { assert(!w->running); ++work_cancels; w->pending = false; }
#define msecs_to_jiffies(x) (x)
static void msleep(unsigned int delay) { (void)delay; }
struct platform_device { int unused; };
typedef int pm_message_t;
static bool uw_lightbar_loaded;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
static int resource_teardowns, last_teardown_event;
static void uniwill_keyboard_prepare_remove(void) {}
static void uw_charging_uninit(struct platform_device *dev) { (void)dev; }
static void uw_battery_uninit(void) {}
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
static void uniwill_leds_restore_state_extern(void) {}
static void uw_charging_priority_write_state(void) {}
static void uw_charging_profile_write_state(void) {}
static int uniwill_write_kbd_bl_enable(u8 enabled) { (void)enabled; return 0; }
struct event { char kind; u16 addr; u8 value; };
static struct event events[16000];
static int event_count, reads, writes;
static u8 ec[65536];
static bool fail_reads, fail_writes;
static int fail_read_at = -1;
static int fail_write_at = -1;
static bool fail_write_lands;
static int fail_write_address = -1;
static void uniwill_leds_remove(struct platform_device *dev) { (void)dev; ++resource_teardowns; last_teardown_event = event_count; }
static int uniwill_read_ec_ram(u16 address, u8 *value) {
    assert(event_count < (int)ARRAY_SIZE(events));
    *value = ec[address]; events[event_count++] = (struct event){'r',address,*value};
    if (fail_reads || reads++ == fail_read_at) { *value = 0xff; return -EREMOTEIO; }
    return 0;
}
static int uniwill_write_ec_ram(u16 address, u8 value) {
    assert(event_count < (int)ARRAY_SIZE(events));
    events[event_count++] = (struct event){'w',address,value};
    bool fault = fail_writes || writes++ == fail_write_at || address == fail_write_address;
    if (!fault || fail_write_lands) ec[address] = value;
    return fault ? -EREMOTEIO : 0;
}
static int uniwill_update_ec_ram(u16 address, u8 clear_mask, u8 set_mask) {
    u8 old; int status = uniwill_read_ec_ram(address, &old);
    return status ? status : uniwill_write_ec_ram(address, (old & ~clear_mask) | set_mask);
}
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @ACTUAL_DRIVER@ */
#pragma GCC diagnostic pop
static const char *selected_case;
static unsigned int assertions, scenarios;
#define CHECK(x) do { ++assertions; if (!(x)) { fprintf(stderr,"%s: line %d: %s\n",selected_case,__LINE__,#x); exit(1); } } while (0)
static u8 original_table[96], original7c5, original7c6, original751;
static void clear_events(void) { event_count = reads = writes = work_queues = work_cancels = 0; }
static void clear_faults(void) { fail_reads = fail_writes = fail_write_lands = false; fail_read_at = fail_write_at = fail_write_address = -1; }
static void reset(void) {
    CHECK(module_references == 0);
    memset(ec,0,sizeof(ec)); memset(&uw_curve_lease,0,sizeof(uw_curve_lease));
    uw_curve_lease.epoch = uw_curve_lease.generation = 1;
    uniwill_device_features = (struct uniwill_device_features_t){.model=0x10,.uniwill_has_universal_ec_fan_control=true};
    dmi_values[0]="MECHREVO"; dmi_values[1]="Kuangshi16Pro Series GM6PX0X"; dmi_values[2]="N.1.09MRO07";
    uniwill_pm_ready=true; uniwill_pm_suspended=false; uniwill_fan_removed=false; uniwill_fan_work_paused=false;
    fans_initialized=universal_fan_manual_active=direct_fan_control_started=direct_fan_control_restart_needed=false;
    direct_fan_control_suspend=legacy_fan_mode_override_blocked=false;
    direct_fan_control_current_value_fan0=direct_fan_control_current_value_fan1=0;
    direct_fan_control_current_value_fan0_suspend_save=29; direct_fan_control_current_value_fan1_suspend_save=61;
    direct_fan_control_restart_delayed_work.pending=direct_fan_control_restart_delayed_work.running=false;
    module_get_allowed=true; resource_teardowns=last_teardown_event=0;
    for (int i=0;i<96;i++) ec[0x0f00+i]=(u8)(13+i*37);
    /* Real preexisting raw states include sentinel/odd/repeated duties. */
    const u8 raw[16]={0,1,3,99,127,199,200,255,255,127,3,1,0,255,255,199};
    memcpy(&ec[0x0f20],raw,16); memcpy(&ec[0x0f50],raw,16);
    ec[0x0f10]=0x31; ec[0x0f40]=0xc7;
    ec[0x07c5]=0xb5; ec[0x07c6]=0xa9; ec[0x0751]=0x12; ec[0x0741]=0x81;
    ec[0x1804]=21; ec[0x1809]=31;
    memcpy(original_table,&ec[0x0f00],96); original7c5=ec[0x07c5]; original7c6=ec[0x07c6]; original751=ec[0x0751];
    clear_faults(); clear_events(); ++scenarios;
}
static struct uw_fan_curve_state_v1 state(u64 owner) {
    struct uw_fan_curve_state_v1 result={0};
    result.control.version=UW_FAN_CURVE_ABI_VERSION; result.control.size=sizeof(result);
    CHECK(uw_fan_curve_get_state(owner,&result)==0); return result;
}
static struct uw_fan_curve_control_v1 control(void) {
    struct uw_fan_curve_state_v1 latest_state=state(0);
    struct uw_fan_curve_control_v1 c=latest_state.control;
    c.size=sizeof(c); c.lease_id=0; c.request_id=41; return c;
}
static struct uw_fan_curve_control_v1 current_control(u64 owner) {
    struct uw_fan_curve_control_v1 c=state(owner).control; c.size=sizeof(c); ++c.request_id; return c;
}
static struct uw_fan_curve_control_v1 acquire(u64 owner) {
    struct uw_fan_curve_control_v1 c=control(); clear_events();
    CHECK(uw_fan_curve_acquire(owner,&c)==0); CHECK(writes==0); CHECK(module_references==1);
    CHECK(c.lease_id!=0); CHECK(uw_curve_lease.owner==owner); return c;
}
static struct uw_fan_curve_apply_v1 request(struct uw_fan_curve_control_v1 c, int variant) {
    struct uw_fan_curve_apply_v1 r={0}; r.control=c; r.control.size=sizeof(r); r.control.request_id+=variant+1;
    for(int f=0;f<2;f++) { for(int i=0;i<15;i++) { r.rise[f][i]=(u8)(20+i*7+f+variant); r.fall[f][i]=(u8)(10+i*7+f); }
        for(int i=0;i<16;i++) r.duty_raw[f][i]=(u8)(i*9+f+variant); }
    r.respective=variant&1; return r;
}
static void activate(u64 owner) { struct uw_fan_curve_apply_v1 r=request(acquire(owner),1); CHECK(uw_fan_curve_apply(owner,&r)==0); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_ACTIVE); }
static void exact_original(void) { CHECK(memcmp(&ec[0x0f00],original_table,96)==0); CHECK(ec[0x07c5]==original7c5); CHECK(ec[0x07c6]==original7c6); CHECK(ec[0x0751]==original751); }
static void no_reserved_or_mode_writes(void) { for(int i=0;i<event_count;i++) if(events[i].kind=='w') CHECK(events[i].addr!=0x0f10 && events[i].addr!=0x0f40 && events[i].addr!=0x0751); }
static void release_owner(u64 owner) { struct uw_fan_curve_control_v1 c=current_control(owner); CHECK(uw_fan_curve_release(owner,&c)==0); CHECK(module_references==0); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_IDLE); }
static void gates(void) {
    for(int op=0;op<9;op++) {
        clear_events(); u64 generation=uw_curve_lease.generation; int status=0;
        switch(op) {
        case 0: status=(int)uw_set_fan(0,110); break;
        case 1: status=(int)uw_set_fan_auto(); break;
        case 2: status=uw_init_fan(); break;
        case 3: status=set_full_fan_mode(true); break;
        case 4: status=set_full_fan_mode(false); break;
        case 5: status=uw_set_mode(0x51); break;
        case 6: status=uw_update_mode(0xb0,0xa0); break;
        case 7: status=uw_write_tdp(0,55); break;
        case 8: status=uw_fan_curve_debug_write(0x0f20,72); break;
        }
        CHECK(status==-EBUSY); CHECK(event_count==0); CHECK(writes==0); CHECK(work_queues==0); CHECK(uw_curve_lease.generation==generation);
    }
    clear_events(); bool old=direct_fan_control_started; direct_fan_control_started=true;
    restart_direct_fan_control_work_handler(&direct_fan_control_restart_delayed_work.work);
    CHECK(event_count==0); CHECK(work_queues==0); CHECK(!direct_fan_control_restart_delayed_work.pending);
    direct_fan_control_started=old;
}
static void acquire_contract(void) {
    for(int mode=0;mode<17;mode++) {
        reset(); struct uw_fan_curve_control_v1 c=control(); int expected=-EBUSY;
        switch(mode) {
        case 0: dmi_values[0]="other"; expected=-ENODEV; break;
        case 1: dmi_values[1]="other"; expected=-ENODEV; break;
        case 2: dmi_values[2]="other"; expected=-ENODEV; break;
        case 3: uniwill_device_features.model=0x11; expected=-ENODEV; break;
        case 4: uniwill_device_features.uniwill_has_universal_ec_fan_control=false; expected=-ENODEV; break;
        case 5: ec[0x0751]|=0x40; break;
        case 6: ec[0x0741]&=~1; break;
        case 7: ec[0x07c6]|=4; break;
        case 8: direct_fan_control_started=true; break;
        case 9: direct_fan_control_restart_needed=true; break;
        case 10: direct_fan_control_suspend=true; break;
        case 11: legacy_fan_mode_override_blocked=true; break;
        case 12: direct_fan_control_restart_delayed_work.pending=true; break;
        case 13: universal_fan_manual_active=true; fans_initialized=false; ec[0x07c6]|=4; break;
        case 14: universal_fan_manual_active=true; fans_initialized=true; ec[0x07c6]|=4; break;
        case 15: module_get_allowed=false; expected=-ENODEV; break;
        case 16: expected=0; break;
        }
        clear_events(); CHECK(uw_fan_curve_acquire(77,&c)==expected); CHECK(writes==0);
        if(expected) { CHECK(module_references==0); CHECK(uw_curve_lease.owner==0); CHECK(!uw_curve_lease.original.valid); }
        else { CHECK(uw_curve_lease.phase==UW_FAN_CURVE_ACQUIRED); CHECK(module_references==1); clear_events(); uw_fan_curve_close(77,c.epoch,c.lease_id); CHECK(event_count==0); CHECK(module_references==0); }
    }
}
static void acquire_read_failures(void) {
    reset(); struct uw_fan_curve_control_v1 c=control(); clear_events(); CHECK(uw_fan_curve_acquire(77,&c)==0); int count=reads;
    CHECK(count==102); uw_fan_curve_close(77,c.epoch,c.lease_id);
    for(int i=0;i<count;i++) { reset(); c=control(); clear_events(); fail_read_at=i; CHECK(uw_fan_curve_acquire(77,&c)==-EREMOTEIO); CHECK(writes==0); CHECK(module_references==0); CHECK(!uw_curve_lease.original.valid); clear_faults(); }
}
static void active_legacy(void) { reset(); activate(77); gates(); release_owner(77); exact_original(); }
static void orphan_recovery(void) {
    reset(); activate(77); struct uw_fan_curve_control_v1 c=current_control(77); fail_reads=true; clear_events();
    uw_fan_curve_close(77,c.epoch,c.lease_id); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED); CHECK(uw_curve_lease.owner==0);
    CHECK(uw_curve_lease.original.valid); CHECK(module_references==1); CHECK(uw_curve_lease.restore_error==-EREMOTEIO); clear_faults(); gates();
    c=control(); clear_events(); CHECK(uw_fan_curve_acquire(88,&c)==0); CHECK(event_count==0); CHECK(module_references==1);
    struct uw_fan_curve_apply_v1 r=request(c,2); CHECK(uw_fan_curve_apply(88,&r)==-EBUSY); CHECK(event_count==0); gates();
    release_owner(88); exact_original(); CHECK(uw_curve_lease.restored_verified); no_reserved_or_mode_writes();
}
static void identity(void) {
    reset(); struct uw_fan_curve_control_v1 c=acquire(77); struct uw_fan_curve_apply_v1 r=request(c,1);
    clear_events(); CHECK(uw_fan_curve_apply(88,&r)==-EPERM); CHECK(event_count==0);
    for(int field=0;field<3;field++) { struct uw_fan_curve_apply_v1 bad=r; if(field==0) ++bad.control.epoch; if(field==1) ++bad.control.generation; if(field==2) ++bad.control.lease_id;
        CHECK(uw_fan_curve_apply(77,&bad)==-ESTALE); CHECK(event_count==0); }
    struct uw_fan_curve_control_v1 duplicate=current_control(77); duplicate.lease_id=0;
    clear_events(); CHECK(uw_fan_curve_acquire(77,&duplicate)==-EBUSY); CHECK(event_count==0);
    CHECK(uw_fan_curve_apply(77,&r)==0); clear_events(); CHECK(uw_fan_curve_apply(77,&r)==-ESTALE); CHECK(event_count==0);
    CHECK(uw_fan_curve_release(77,&c)==-ESTALE); CHECK(event_count==0);
    uw_fan_curve_close(77,c.epoch+1,c.lease_id); uw_fan_curve_close(77,c.epoch,c.lease_id+1); uw_fan_curve_close(88,c.epoch,c.lease_id);
    CHECK(event_count==0); CHECK(module_references==1); release_owner(77);
    c=control(); CHECK(uw_write_tdp(0,55)==0); clear_events(); CHECK(uw_fan_curve_acquire(77,&c)==-ESTALE); CHECK(event_count==0);
}
static void abi_validation(void) {
    reset(); CHECK(sizeof(struct uw_fan_curve_control_v1)==64); CHECK(sizeof(struct uw_fan_curve_apply_v1)==160); CHECK(sizeof(struct uw_fan_curve_state_v1)==320);
    struct uw_fan_curve_control_v1 c=control();
    for(int i=0;i<8;i++) { struct uw_fan_curve_control_v1 bad=c;
        if(i==0) ++bad.version;
        if(i==1) ++bad.size;
        if(i==2) bad.flags=1;
        if(i==3) bad.reserved0=1;
        if(i>=4) bad.reserved[i-4]=1;
        clear_events(); CHECK(uw_fan_curve_acquire(77,&bad)==-EINVAL); CHECK(event_count==0); }
    clear_events(); CHECK(uw_fan_curve_acquire(0,&c)==-EINVAL); CHECK(event_count==0);
    c=acquire(77); struct uw_fan_curve_apply_v1 r=request(c,1);
    for(int i=0;i<3;i++) { r.reserved[i]=1; clear_events(); CHECK(uw_fan_curve_apply(77,&r)==-EINVAL); CHECK(event_count==0); r.reserved[i]=0; }
    for(int f=0;f<2;f++) for(int i=0;i<16;i++) { u8 old=r.duty_raw[f][i]; r.duty_raw[f][i]=201; CHECK(uw_fan_curve_apply(77,&r)==-EINVAL); CHECK(event_count==0); r.duty_raw[f][i]=old; }
    CHECK(uw_fan_curve_get_state(77,NULL)==-EINVAL); CHECK(uw_fan_curve_apply(77,NULL)==-EINVAL); CHECK(uw_fan_curve_acquire(77,NULL)==-EINVAL); CHECK(uw_fan_curve_release(77,NULL)==-EINVAL);
    release_owner(77);
}
static void raw_restore(void) {
    for(int manual=0;manual<2;manual++) {
        reset(); if(manual) { universal_fan_manual_active=fans_initialized=true; ec[0x07c6]|=4; original7c6=ec[0x07c6];
            direct_fan_control_current_value_fan0=57; direct_fan_control_current_value_fan1=79; ec[0x0f20]=57; ec[0x0f50]=79; memcpy(original_table,&ec[0x0f00],96); }
        struct uw_fan_curve_control_v1 c=acquire(77); struct uw_fan_curve_state_v1 s=state(77);
        CHECK((s.state_flags&(UW_FAN_CURVE_STATE_OWNER|UW_FAN_CURVE_STATE_ORIGINAL_VALID))==(UW_FAN_CURVE_STATE_OWNER|UW_FAN_CURVE_STATE_ORIGINAL_VALID));
        CHECK(memcmp(s.original_table,original_table,96)==0); CHECK(s.original_targets[2]==29 && s.original_targets[3]==61);
        for(int variant=1;variant<4;variant++) { struct uw_fan_curve_apply_v1 r=request(c,variant); CHECK(uw_fan_curve_apply(77,&r)==0); c=current_control(77); CHECK(memcmp(uw_curve_lease.original.table,original_table,96)==0); }
        clear_events(); CHECK(uw_fan_curve_release(77,&c)==0); CHECK(module_references==0); exact_original(); no_reserved_or_mode_writes();
        CHECK(universal_fan_manual_active==(bool)manual); CHECK(fans_initialized==(bool)manual);
        CHECK(direct_fan_control_current_value_fan0==(manual?57:0)); CHECK(direct_fan_control_current_value_fan1==(manual?79:0));
        CHECK(direct_fan_control_current_value_fan0_suspend_save==29 && direct_fan_control_current_value_fan1_suspend_save==61);
        CHECK(ec[0x1804]==(manual?57:21)); CHECK(ec[0x1809]==(manual?79:31));
        s=state(77); CHECK(s.phase==UW_FAN_CURVE_IDLE); CHECK(s.state_flags&UW_FAN_CURVE_STATE_RESTORED_VERIFIED); CHECK(!(s.state_flags&UW_FAN_CURVE_STATE_ORIGINAL_VALID));
    }
}
static void expected_mismatch(void) {
    const u16 addresses[]={0x0f00,0x0f10,0x07c5,0x07c6,0x0751};
    for(unsigned int i=0;i<ARRAY_SIZE(addresses);i++) { reset(); struct uw_fan_curve_control_v1 c=acquire(77); struct uw_fan_curve_apply_v1 r=request(c,1);
        ec[addresses[i]]^=i==2?0x80:i==3?4:1; clear_events(); CHECK(uw_fan_curve_apply(77,&r)==-EAGAIN); CHECK(writes==0); CHECK(!uw_curve_lease.applied);
        clear_events(); uw_fan_curve_close(77,c.epoch,c.lease_id); CHECK(event_count==0); CHECK(module_references==0); }
}
static void failed_unapplied_release(void) {
    for(int variant=0;variant<2;variant++) {
        reset(); struct uw_fan_curve_control_v1 c=acquire(77); struct uw_fan_curve_apply_v1 r=request(c,1);
        clear_events();
        if(variant==0) ec[0x0f00]^=1;
        else fail_write_at=0;
        CHECK(uw_fan_curve_apply(77,&r)==(variant?-EREMOTEIO:-EAGAIN));
        CHECK(uw_curve_lease.phase==UW_FAN_CURVE_ACQUIRED); CHECK(!uw_curve_lease.applied);
        CHECK(uw_curve_lease.last_error!=0); clear_faults(); c=current_control(77); clear_events();
        CHECK(uw_fan_curve_release(77,&c)==0); CHECK(event_count==0); CHECK(module_references==0);
        struct uw_fan_curve_state_v1 s=state(77); CHECK(s.phase==UW_FAN_CURVE_IDLE); CHECK(s.last_error==0); CHECK(s.restore_error==0);
    }
}
static void rebind_unqualified(void) {
    for(int variant=0;variant<3;variant++) {
        reset(); activate(77); struct uw_fan_curve_control_v1 c=current_control(77);
        fail_reads=true; uw_fan_curve_close(77,c.epoch,c.lease_id); clear_faults();
        CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED); CHECK(module_references==1);
        if(variant==0) uniwill_device_features.uniwill_has_universal_ec_fan_control=false;
        if(variant==1) dmi_values[1]="different board";
        if(variant==2) dmi_values[2]="different BIOS";
        clear_events(); CHECK(uw_fan_control_start()==-ENODEV); CHECK(writes==0);
        CHECK(uw_curve_lease.original.valid); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED); CHECK(module_references==1);
        uniwill_device_features.uniwill_has_universal_ec_fan_control=true;
        dmi_values[1]="Kuangshi16Pro Series GM6PX0X"; dmi_values[2]="N.1.09MRO07";
        uniwill_fan_removed=false; uniwill_pm_ready=true; c=control(); CHECK(uw_fan_curve_acquire(88,&c)==0); release_owner(88); exact_original();
    }
}
static void pm_lifecycle(void) {
    reset(); activate(77); struct uw_fan_curve_control_v1 old=current_control(77); clear_events();
    CHECK(uniwill_keyboard_suspend(NULL,0)==0); CHECK(uniwill_pm_suspended); CHECK(module_references==0); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_IDLE); exact_original();
    clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0); CHECK(uniwill_keyboard_resume(NULL)==0); CHECK(!uniwill_pm_suspended);
    struct uw_fan_curve_control_v1 next=acquire(88); clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0); CHECK(uw_curve_lease.owner==88);
    uw_fan_curve_close(88,next.epoch,next.lease_id); CHECK(module_references==0);
}
static void pm_recovery(void) {
    reset(); activate(77); fail_reads=true; CHECK(uniwill_keyboard_suspend(NULL,0)==-EREMOTEIO);
    CHECK(!uniwill_pm_suspended); CHECK(uw_curve_lease.owner==77); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED); CHECK(module_references==1); CHECK(uw_curve_lease.original.valid);
    clear_faults(); gates(); release_owner(77); exact_original();
}
static void metadata_transport_failure(void) {
    reset(); activate(77); fail_reads=true; clear_events(); struct uw_fan_curve_state_v1 s=state(77);
    CHECK(s.phase==UW_FAN_CURVE_ACTIVE); CHECK(s.state_flags&UW_FAN_CURVE_STATE_ORIGINAL_VALID);
    CHECK(s.state_flags&UW_FAN_CURVE_STATE_OWNER); CHECK(!(s.state_flags&UW_FAN_CURVE_STATE_LIVE_VALID));
    CHECK(s.live_error==-EREMOTEIO); CHECK(s.last_error==0); CHECK(s.restore_error==0); CHECK(writes==0);
    CHECK(memcmp(s.original_table,original_table,96)==0); clear_faults(); release_owner(77);
    fail_reads=true; s=state(0); CHECK(s.state_flags&UW_FAN_CURVE_STATE_RESTORED_VERIFIED);
    CHECK(s.live_error==-EREMOTEIO); CHECK(s.last_error==0); CHECK(s.restore_error==0); clear_faults();
    dmi_values[0]="other"; clear_events(); s=state(0); CHECK(!(s.capabilities&UW_FAN_CURVE_CAP_QUALIFIED));
    CHECK(!(s.state_flags&UW_FAN_CURVE_STATE_LIVE_VALID)); CHECK(event_count==0);
}
static void manual_target_restore_failure(void) {
    for(int address=0x1804;address<=0x1809;address+=5) for(int landed=0;landed<2;landed++) {
        reset(); universal_fan_manual_active=fans_initialized=true; ec[0x07c6]|=4; original7c6=ec[0x07c6];
        direct_fan_control_current_value_fan0=57; direct_fan_control_current_value_fan1=79;
        ec[0x0f20]=57; ec[0x0f50]=79; memcpy(original_table,&ec[0x0f00],96); activate(77);
        struct uw_fan_curve_control_v1 c=current_control(77); clear_events(); fail_write_address=address; fail_write_lands=landed;
        CHECK(uw_fan_curve_release(77,&c)==-EREMOTEIO); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED);
        CHECK(uw_curve_lease.original.valid); CHECK(module_references==1); CHECK(!(ec[0x07c6]&4));
        clear_faults(); gates(); release_owner(77); exact_original(); CHECK(ec[0x1804]==57 && ec[0x1809]==79);
    }
}
static void failed_remove_recovery(void) {
    reset(); activate(77); struct uw_fan_curve_control_v1 old=current_control(77); fail_reads=true;
    uniwill_keyboard_remove(NULL); CHECK(uniwill_fan_removed); CHECK(resource_teardowns==1); CHECK(ec[0x0741]==0);
    CHECK(uw_curve_lease.phase==UW_FAN_CURVE_RECOVERY_REQUIRED); CHECK(uw_curve_lease.original.valid);
    CHECK(uw_curve_lease.owner==0); CHECK(module_references==1); CHECK(uw_curve_lease.epoch!=old.epoch);
    clear_faults(); clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0);
    CHECK(uw_fan_control_start()==0); CHECK(module_references==0); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_IDLE);
    CHECK(memcmp(&ec[0x0f00],original_table,96)==0); CHECK(ec[0x07c5]==original7c5 && ec[0x07c6]==original7c6);
    clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0);
}
static void abort_preserves_errno(void) {
    for(int disable=0;disable<2;disable++) for(int fault=0;fault<2;fault++) {
        reset(); uniwill_pm_ready=false; fail_writes=fault; clear_events();
        CHECK(uniwill_keyboard_abort_probe(-ENXIO,disable)==-ENXIO);
        CHECK(uniwill_fan_removed); CHECK(!uniwill_pm_ready); CHECK(module_references==0);
        for(int i=0;i<event_count;i++) CHECK(events[i].kind=='w' && events[i].addr==0x0741);
        CHECK(event_count==disable); clear_faults();
    }
}
static void remove_lifecycle(void) {
    reset(); activate(77); struct uw_fan_curve_control_v1 old=current_control(77); clear_events(); uniwill_keyboard_remove(NULL);
    CHECK(uniwill_fan_removed); CHECK(module_references==0); CHECK(uw_curve_lease.phase==UW_FAN_CURVE_IDLE); CHECK(resource_teardowns==1);
    CHECK(memcmp(&ec[0x0f00],original_table,96)==0); CHECK(ec[0x07c5]==original7c5 && ec[0x07c6]==original7c6); CHECK(ec[0x0741]==0);
    bool saw_table=false; for(int i=0;i<last_teardown_event;i++) if(events[i].kind=='w' && events[i].addr>=0x0f00 && events[i].addr<=0x0f5f) saw_table=true;
    CHECK(saw_table); clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0);
    CHECK(uw_set_mode(0)==-ENODEV); CHECK(event_count==0);
    CHECK(uw_fan_control_start()==0); CHECK(uw_curve_lease.epoch!=old.epoch); uniwill_pm_ready=true; ec[0x0741]=1;
    struct uw_fan_curve_control_v1 next=acquire(88); clear_events(); uw_fan_curve_close(77,old.epoch,old.lease_id); CHECK(event_count==0); CHECK(uw_curve_lease.owner==88);
    uw_fan_curve_close(88,next.epoch,next.lease_id); CHECK(module_references==0);
}
int main(int argc,char **argv) {
    if(argc!=2) return 2;
    selected_case=argv[1];
    if(!strcmp(argv[1],"acquire_contract")) acquire_contract();
    else if(!strcmp(argv[1],"acquire_read_failures")) acquire_read_failures();
    else if(!strcmp(argv[1],"active_legacy")) active_legacy();
    else if(!strcmp(argv[1],"orphan_recovery")) orphan_recovery();
    else if(!strcmp(argv[1],"identity")) identity();
    else if(!strcmp(argv[1],"abi_validation")) abi_validation();
    else if(!strcmp(argv[1],"raw_restore")) raw_restore();
    else if(!strcmp(argv[1],"expected_mismatch")) expected_mismatch();
    else if(!strcmp(argv[1],"failed_unapplied_release")) failed_unapplied_release();
    else if(!strcmp(argv[1],"rebind_unqualified")) rebind_unqualified();
    else if(!strcmp(argv[1],"pm_lifecycle")) pm_lifecycle();
    else if(!strcmp(argv[1],"pm_recovery")) pm_recovery();
    else if(!strcmp(argv[1],"metadata_transport_failure")) metadata_transport_failure();
    else if(!strcmp(argv[1],"manual_target_restore_failure")) manual_target_restore_failure();
    else if(!strcmp(argv[1],"failed_remove_recovery")) failed_remove_recovery();
    else if(!strcmp(argv[1],"abort_preserves_errno")) abort_preserves_errno();
    else if(!strcmp(argv[1],"remove_lifecycle")) remove_lifecycle();
    else return 2;
    printf("{\"case\":\"%s\",\"scenarios\":%u,\"assertions\":%u}\n",selected_case,scenarios,assertions);
    return 0;
}
