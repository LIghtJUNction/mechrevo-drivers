/* Real driver functions, mocked EC/workqueue, deterministic pthread barriers. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
struct uniwill_device_features_t {
    bool uniwill_has_universal_ec_fan_control;
    bool uniwill_custom_profile_mode_needed;
    bool uniwill_profile_v1;
};
static struct uniwill_device_features_t uniwill_device_features;
struct platform_device { struct { struct { int unused; } kobj; } dev; };
typedef int pm_message_t;
static bool uw_charging_prio_loaded, uw_charging_profile_loaded, uw_lightbar_loaded;
static __attribute__((unused)) int uw_charging_prio_attr_group, uw_charging_profile_attr_group;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
static void sysfs_remove_group(void *kobj, void *group) { (void)kobj; (void)group; }
/* Charging lifecycle is exercised with its real implementation separately. */
static __attribute__((unused)) void uw_charging_uninit(struct platform_device *dev)
{
    (void)dev;
    if (uw_charging_prio_loaded) sysfs_remove_group(NULL, &uw_charging_prio_attr_group);
    if (uw_charging_profile_loaded) sysfs_remove_group(NULL, &uw_charging_profile_attr_group);
}
static void uw_battery_uninit(void) {}
/* Deferred key-work ordering is exercised by uniwill_key_work_runtime.c. */

static void uniwill_leds_remove(struct platform_device *dev);
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
static void uniwill_leds_restore_state_extern(void);
static void uw_charging_priority_write_state(void);
static void uw_charging_profile_write_state(void);

#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define EXPORT_SYMBOL(name)
#define pr_debug(...) do {} while (0)
#define pr_err(format, error) do { ++reported_errors; last_reported_error = (error); } while (0)
#define pr_info(...) do {} while (0)
#define lockdep_assert_held(lock) do {} while (0)

static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static _Thread_local int actor;
static bool complete[4], waiting[4];
static int injected_errno = -ETIMEDOUT, faults;
static char fault_kind;
static u16 fault_address;
static bool chain_fault;
static char chained_kind;
static u16 chained_address;
static int chained_errno;
/* Only the next failing profile write lands; recovery has its own fault. */
static bool land_profile_failure;
static int result[4];
struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER }
static __attribute__((unused)) void mutex_lock(struct mutex *lock)
{
    int status = pthread_mutex_trylock(&lock->native);
    if (status == EBUSY) {
        pthread_mutex_lock(&harness);
        waiting[actor] = true;
        pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&harness);
        status = pthread_mutex_lock(&lock->native);
    }
    assert(status == 0);
}
static __attribute__((unused)) void mutex_unlock(struct mutex *lock) { assert(pthread_mutex_unlock(&lock->native) == 0); }

struct work_struct { int unused; };
struct delayed_work { struct work_struct work; bool pending, running; };
#define DECLARE_DELAYED_WORK(name, handler) struct delayed_work name
static bool schedule_delayed_work(struct delayed_work *work, unsigned int delay)
{
    (void)delay;
    pthread_mutex_lock(&harness);
    bool queued = !work->pending;
    work->pending = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    return queued;
}
static void cancel_delayed_work_sync(struct delayed_work *work)
{
    pthread_mutex_lock(&harness);
    while (work->running) {
        waiting[actor] = true;
        pthread_cond_broadcast(&changed);
        pthread_cond_wait(&changed, &harness);
    }
    work->pending = false;
    pthread_mutex_unlock(&harness);
}
#define msecs_to_jiffies(value) (value)
static void msleep(unsigned int value) { (void)value; }

struct event { int actor; char kind; u16 address; u8 value; };
static struct event events[2048];
static int event_count, table_writes;
static u8 ec[UINT16_MAX + 1u];
static bool paused, release_pause;
static int pause_actor = 1, pause_occurrence = 1, seen;
static char pause_kind = 'w';
static u16 pause_address;
static void uniwill_leds_remove(struct platform_device *dev)
{
    (void)dev;
    pthread_mutex_lock(&harness);
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, 'L', 0, 0};
    pthread_mutex_unlock(&harness);
}
static void pause_if_selected(char kind, u16 address)
{
    if (actor == pause_actor && kind == pause_kind && address == pause_address &&
        ++seen == pause_occurrence) {
        paused = true;
        pthread_cond_broadcast(&changed);
        while (!release_pause)
            pthread_cond_wait(&changed, &harness);
    }
}
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
    pthread_mutex_lock(&harness);
    /* Pause after sampling to expose a stale read-modify-write. */
    bool fail = faults && fault_kind == 'r' && fault_address == address;
    if (fail) --faults;
    int error = fail ? injected_errno : 0;
    if (!fail) *value = ec[address];
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, 'r', address, ec[address]};
    pause_if_selected('r', address);
    if (fail && chain_fault) {
        fault_kind = chained_kind; fault_address = chained_address;
        injected_errno = chained_errno; faults = 1; chain_fault = false;
    }
    pthread_mutex_unlock(&harness);
    return error;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
    pthread_mutex_lock(&harness);
    /* Pause before committing, so competing requests expose lost updates. */
    pause_if_selected('w', address);
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, 'w', address, value};
    bool fail = faults && fault_kind == 'w' && fault_address == address;
    if (fail) --faults;
    int error = fail ? injected_errno : 0;
    if (!fail || (land_profile_failure && address == 0x0727)) ec[address] = value;
    if (fail && address == 0x0727) land_profile_failure = false;
    if (address >= 0x0f00 && address <= 0x0f5f)
        ++table_writes;
    if (fail && chain_fault) {
        fault_kind = chained_kind; fault_address = chained_address;
        injected_errno = chained_errno; faults = 1; chain_fault = false;
    }
    pthread_mutex_unlock(&harness);
    return error;
}


/* Service double for the probe/PM fixture's new atomic update entry point.
 * Shared-register concurrency runs the actual foundation in LED transaction tests. */
static int __attribute__((unused)) uniwill_update_ec_ram(unsigned int address, u8 clear, u8 set)
{
    u8 data;
    int result = uniwill_read_ec_ram(address, &data);
    if (result) return result;
    return uniwill_write_ec_ram(address, (data & ~clear) | set);
}

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#define UW_EC_REG_KBD_BL_STATUS 0x078c
#define UW_EC_REG_CUSTOM_PROFILE 0x0727
#define UNIWILL_OSD_MODE_CHANGE_KEY_EVENT 0x0b0
#define UNIWILL_OSD_DC_ADAPTER_CHANGE 0x0ab
#define UNIWILL_KEY_KBDILLUMTOGGLE 0x0b9
#define UNIWILL_OSD_KB_LED_LEVEL0 0x03b
#define UNIWILL_OSD_KB_LED_LEVEL1 0x03c
#define UNIWILL_OSD_KB_LED_LEVEL2 0x03d
#define UNIWILL_OSD_KB_LED_LEVEL3 0x03e
#define UNIWILL_OSD_KB_LED_LEVEL4 0x03f
#define KEY_LEFTMETA 1
#define KEY_LEFTALT 2
#define KEY_F6 3
#define TUXEDO_ERROR(...) do { ++reported_errors; } while (0)
#define TUXEDO_DEBUG(...) do {} while (0)
#define fallthrough __attribute__((fallthrough))
#define __always_unused __attribute__((unused))
#define unlikely(value) (value)
#define ARRAY_SIZE(value) (sizeof(value) / sizeof((value)[0]))
#define I8042_STR_AUXDATA 0x20
struct serio { int unused; };
struct input_dev { int unused; };
static struct { struct input_dev *input_device; } uniwill_keyboard_driver;
static int reported_errors, last_reported_error;
static int input_events;
static void input_report_key(struct input_dev *dev, int key, int pressed)
{ (void)dev; (void)key; (void)pressed; ++input_events; }
static void input_sync(struct input_dev *dev) { (void)dev; ++input_events; }
static bool sparse_keymap_report_known_event(struct input_dev *dev, u32 code, int value, bool autorelease)
{ (void)dev; (void)code; (void)value; (void)autorelease; ++input_events; return true; }
static bool uniwill_leds_notify_brightness_change_extern(void) { ++input_events; return true; }
static void resource_event(char kind)
{
    pthread_mutex_lock(&harness);
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, kind, 0, 0};
    pause_if_selected(kind, 0);
    pthread_mutex_unlock(&harness);
}
static void uniwill_leds_restore_state_extern(void) { resource_event('L'); }
static void uw_charging_priority_write_state(void) { resource_event('P'); }
static void uw_charging_profile_write_state(void) { resource_event('C'); }
static void set_rom_id(void) {}
static struct uniwill_device_features_t *uniwill_get_device_features(void)
{ return &uniwill_device_features; }
static void uniwill_leds_init(struct platform_device *dev) { (void)dev; }
static int uw_lightbar_init(struct platform_device *dev) { (void)dev; return -ENODEV; }
#define INIT_STUB(name) static void name(struct platform_device *dev) { (void)dev; }
INIT_STUB(uw_charging_priority_init)
INIT_STUB(uw_charging_profile_init)
INIT_STUB(uw_ac_auto_boot_init)
INIT_STUB(uw_usb_powershare_init)
INIT_STUB(uw_mini_led_local_dimming_init)
static void uw_show_hidden_bios_options(void) {}
static void uw_battery_init(void) {}
static struct work_struct uniwill_key_event_work;
static bool schedule_work(struct work_struct *work) { (void)work; return true; }
static void cancel_work_sync(struct work_struct *work) { (void)work; }
typedef bool (*filter_fn)(unsigned char, unsigned char, struct serio *, void *);
static int i8042_install_filter(filter_fn filter, void *context)
{ (void)filter; (void)context; return 0; }
static int i8042_remove_filter(filter_fn filter) { (void)filter; return 0; }
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop

/* Baseline lacks the two serialized mode exports and reprobe helper. */

enum operation { ADAPTER, SUSPEND, RESUME, REMOVE };
static enum operation operations[4];
static struct platform_device platform;
static void *run(void *arg)
{
    actor = (int)(intptr_t)arg;
    switch (operations[actor]) {
    case ADAPTER: uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); result[actor] = 0; break;
    case SUSPEND: result[actor] = uniwill_keyboard_suspend(&platform, 0); break;
    case RESUME: result[actor] = uniwill_keyboard_resume(&platform); break;
    case REMOVE: uniwill_keyboard_remove(&platform); result[actor] = 0; break;
    }
    pthread_mutex_lock(&harness);
    complete[actor] = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    return NULL;
}
static pthread_t start(int id, enum operation op)
{
    pthread_t thread;
    operations[id] = op;
    assert(pthread_create(&thread, NULL, run, (void *)(intptr_t)id) == 0);
    return thread;
}
static void wait_until(bool *condition)
{
    struct timespec end;
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0); end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!*condition) assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    pthread_mutex_unlock(&harness);
}
static void interleave(enum operation first, enum operation second)
{
    pthread_t one = start(1, first); wait_until(&paused);
    pthread_t two = start(2, second);
    struct timespec end;
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0); end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!complete[2] && !waiting[2]) assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    release_pause = true; pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
    assert(result[1] == 0 && result[2] == 0);
}
static void clear_events(void)
{
    event_count = table_writes = 0;
    paused = release_pause = false; seen = 0;
    memset(complete, 0, sizeof(complete)); memset(waiting, 0, sizeof(waiting));
}
static void begin(bool universal)
{
    actor = 0; faults = 0; chain_fault = false; land_profile_failure = false;
    uniwill_device_features.uniwill_has_universal_ec_fan_control = universal;
    uniwill_device_features.uniwill_custom_profile_mode_needed = true;
    ec[0x0727] = 0xe5; ec[0x078c] = 0x81;
    assert(uniwill_keyboard_probe(&platform) == 0);
    ec[0x0751] = 0x11; clear_events();
}
static void run_worker(void)
{
    assert(direct_fan_control_restart_delayed_work.pending);
    direct_fan_control_restart_delayed_work.pending = false;
    restart_direct_fan_control_work_handler(&direct_fan_control_restart_delayed_work.work);
}
static void manual(void)
{
    assert(uw_set_fan(0, 83) == 0 && uw_set_fan(1, 97) == 0);
    if (direct_fan_control_restart_delayed_work.pending) run_worker();
    clear_events();
}
static void fault_once(char kind, u16 address, int error)
{ fault_kind = kind; fault_address = address; faults = 1; injected_errno = error; }
static int actor_events(int who)
{
    int count = 0; for (int i = 0; i < event_count; ++i) count += events[i].actor == who;
    return count;
}
static void assert_serial_actors(void)
{
    bool saw_second = false;
    for (int i = 0; i < event_count; ++i) {
        if (events[i].actor == 2) saw_second = true;
        if (events[i].actor == 1) assert(!saw_second);
    }
    assert(saw_second);
}
static int find_event(char kind, u16 address)
{
    for (int i = 0; i < event_count; ++i)
        if (events[i].kind == kind && events[i].address == address) return i;
    return -1;
}
static void assert_event_order(char first_kind, u16 first_address, char second_kind, u16 second_address)
{
    int first = find_event(first_kind, first_address);
    int second = find_event(second_kind, second_address);
    assert(first >= 0 && second >= 0 && first < second);
}
static int profile_accesses(char kind)
{
    int count = 0;
    for (int i = 0; i < event_count; ++i)
        count += events[i].kind == kind && events[i].address == 0x0727;
    return count;
}
static void assert_abort_fans(void)
{
    assert(!uniwill_pm_suspended);
    assert(direct_fan_control_current_value_fan0 == 83);
    assert(direct_fan_control_current_value_fan1 == 97);
    assert(!direct_fan_control_suspend);
    assert(ec[0x1804] == 83 && ec[0x1809] == 97);
}
int main(int argc, char **argv)
{
    assert(argc == 2); const char *test = argv[1]; actor = 0;
    if (!strcmp(test, "lifecycle")) {
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
        assert(event_count == 0);
        assert(uw_fan_control_start() == 0); clear_events();
        assert((int)uw_set_fan(0, 83) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
        assert(uw_init_fan() == -ENODEV && set_full_fan_mode(true) == -ENODEV);
        assert(uw_set_mode(0x51) == -ENODEV && uw_update_mode(0xb0, 0xa0) == -ENODEV);
        assert(uniwill_keyboard_suspend(&platform, 0) == -ENODEV);
        assert(uniwill_keyboard_resume(&platform) == -ENODEV);
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count == 0);
        fault_once('w', 0x0751, -EIO);
        assert(uniwill_keyboard_probe(&platform) == -EIO); clear_events();
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count == 0);
        begin(false); uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count > 0);
        uniwill_keyboard_remove(&platform); clear_events();
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count == 0);
        begin(false); uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count > 0);
        return 0;
    }
    begin(false);
    if (!strcmp(test, "profile_abort_matrix")) {
        const int errors[] = {-EIO, -ETIMEDOUT, -ENODEV};
        for (int universal = 0; universal < 2; ++universal)
            for (int enabled = 0; enabled < 2; ++enabled)
                for (int write = 0; write < 2; ++write)
                    for (int landed = 0; landed <= write; ++landed)
                        for (unsigned int e = 0; e < ARRAY_SIZE(errors); ++e) {
                            begin(universal); manual(); ec[0x0727] = enabled ? 0xe5 : 0xa5;
                            int before_errors = reported_errors;
                            fault_once(write ? 'w' : 'r', 0x0727, errors[e]);
                            land_profile_failure = landed;
                            assert(uniwill_keyboard_suspend(&platform, 0) == errors[e]);
                            assert(ec[0x0727] == (enabled ? 0xe5 : 0xa5));
                            assert(profile_accesses('w') == (write ? 1 + enabled : 0));
                            assert(profile_accesses('r') == 1 + (write && enabled));
                            assert(reported_errors == before_errors);
                            assert_abort_fans();
                            /* No resume callback is needed for a later real PM cycle. */
                            assert(uniwill_keyboard_suspend(&platform, 0) == 0);
                            assert(uniwill_keyboard_resume(&platform) == 0);
                            assert_abort_fans();
                        }
    } else if (!strcmp(test, "profile_abort_cleanup")) {
        for (int universal = 0; universal < 2; ++universal)
            for (int write = 0; write < 2; ++write) {
                begin(universal); manual(); int before_errors = reported_errors;
                fault_once('w', 0x0727, -ETIMEDOUT); land_profile_failure = true;
                chain_fault = true; chained_kind = write ? 'w' : 'r';
                chained_address = 0x0727; chained_errno = -ENOSPC;
                assert(uniwill_keyboard_suspend(&platform, 0) == -ETIMEDOUT);
                assert(reported_errors == before_errors + 1 && last_reported_error == -ENOSPC);
                assert(ec[0x0727] == 0xa5);
                assert(profile_accesses('w') == 1 + write);
                assert_abort_fans();
                uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
                assert(ec[0x0727] == 0xe5);
                assert(uniwill_keyboard_suspend(&platform, 0) == 0);
                assert(uniwill_keyboard_resume(&platform) == 0);
                assert_abort_fans();
            }
    } else if (!strcmp(test, "profile_success_and_qualification")) {
        for (int universal = 0; universal < 2; ++universal)
            for (int needed = 0; needed < 2; ++needed)
                for (int enabled = 0; enabled < 2; ++enabled) {
                    begin(universal); manual();
                    uniwill_device_features.uniwill_custom_profile_mode_needed = needed;
                    ec[0x0727] = enabled ? 0xe5 : 0xa5;
                    assert(uniwill_keyboard_suspend(&platform, 0) == 0);
                    assert(ec[0x0727] == (needed ? 0xa5 : (enabled ? 0xe5 : 0xa5)));
                    assert(profile_accesses('r') == needed && profile_accesses('w') == needed);
                    assert(uniwill_keyboard_resume(&platform) == 0);
                    assert_abort_fans();
                }
    } else if (!strcmp(test, "profile_abort_concurrency")) {
        for (int universal = 0; universal < 2; ++universal) {
            begin(universal); manual();
            fault_once('w', 0x0727, -EIO); land_profile_failure = true;
            /* Hold the rollback read, after the failed clear already landed. */
            pause_actor = 1; pause_kind = 'r'; pause_address = 0x0727; pause_occurrence = 2;
            pthread_t one = start(1, SUSPEND); wait_until(&paused);
            assert(ec[0x0727] == 0xa5);
            pthread_t two = start(2, ADAPTER); wait_until(&waiting[2]);
            pthread_mutex_lock(&harness);
            assert(actor_events(2) == 0 && !complete[2]);
            release_pause = true; pthread_cond_broadcast(&changed);
            pthread_mutex_unlock(&harness);
            assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
            assert(result[1] == -EIO && result[2] == 0);
            assert(ec[0x0727] == 0xe5); assert_serial_actors(); assert_abort_fans();
            pause_occurrence = 1;
        }
    } else if (!strcmp(test, "adapter_suspend")) {
        pause_kind = 'r'; pause_address = 0x0727;
        interleave(ADAPTER, SUSPEND);
        assert(ec[0x0727] == 0xa5);
    } else if (!strcmp(test, "suspend_adapter")) {
        pause_kind = 'r'; pause_address = 0x0727;
        interleave(SUSPEND, ADAPTER);
        assert(ec[0x0727] == 0xa5 && actor_events(2) == 0);
    } else if (!strcmp(test, "late_adapter")) {
        for (int universal = 0; universal < 2; ++universal) {
            for (int use_manual = 0; use_manual < 2; ++use_manual) {
                begin(universal); if (use_manual) manual();
                assert(uniwill_keyboard_suspend(&platform, 0) == 0);
                clear_events(); uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
                assert(event_count == 0 && ec[0x0727] == 0xa5);
                assert(uniwill_keyboard_resume(&platform) == 0);
            }
        }
    } else if (!strcmp(test, "suspended_commands")) {
        for (int universal = 0; universal < 2; ++universal) {
            begin(universal); assert(uniwill_keyboard_suspend(&platform, 0) == 0); clear_events();
            assert((int)uw_set_fan(0, 83) == -EBUSY && (int)uw_set_fan_auto() == -EBUSY);
            assert(uw_init_fan() == -EBUSY && set_full_fan_mode(true) == -EBUSY);
            assert(uw_set_mode(0x51) == -EBUSY && uw_update_mode(0xb0, 0xa0) == -EBUSY);
            assert(event_count == 0 && !direct_fan_control_restart_delayed_work.pending);
            assert(uniwill_keyboard_resume(&platform) == 0);
            assert(uw_set_fan(0, 83) == 0 && (int)uw_set_fan_auto() == 0);
        }
    } else if (!strcmp(test, "pm_order")) {
        manual(); assert(uniwill_keyboard_suspend(&platform, 0) == 0);
        assert_event_order('w', 0x0751, 'w', 0x0727);
        assert_event_order('w', 0x0727, 'w', 0x078c);
        clear_events(); assert(uniwill_keyboard_resume(&platform) == 0);
        assert_event_order('w', 0x1809, 'w', 0x0727);
        assert_event_order('w', 0x0727, 'L', 0);
        assert_event_order('L', 0, 'w', 0x078c);
        assert_event_order('w', 0x078c, 'P', 0);
        assert_event_order('P', 0, 'C', 0);
        run_worker(); assert(ec[0x1804] == 83 && ec[0x1809] == 97);
    } else if (!strcmp(test, "suspend_backlight_failure")) {
        for (int enabled = 0; enabled < 2; ++enabled) {
            for (int write = 0; write < 2; ++write) {
                begin(false); manual(); ec[0x0727] = enabled ? 0xe5 : 0xa5;
                fault_once(write ? 'w' : 'r', 0x078c, -ETIMEDOUT);
                assert(uniwill_keyboard_suspend(&platform, 0) == -ETIMEDOUT);
                assert(ec[0x0727] == (enabled ? 0xe5 : 0xa5));
                assert(direct_fan_control_current_value_fan0_suspend_save == 83);
                assert(direct_fan_control_current_value_fan1_suspend_save == 97);
                run_worker(); assert(ec[0x1804] == 83 && ec[0x1809] == 97);
                assert(uw_set_fan(0, 110) == 0);
            }
        }
    } else if (!strcmp(test, "suspend_cleanup_failure")) {
        for (int write = 0; write < 2; ++write) {
            begin(false); manual(); int before_errors = reported_errors;
            fault_once('r', 0x078c, -ETIMEDOUT);
            chain_fault = true; chained_kind = write ? 'w' : 'r';
            chained_address = 0x0727; chained_errno = -EIO;
            assert(uniwill_keyboard_suspend(&platform, 0) == -ETIMEDOUT);
            assert(reported_errors == before_errors + 1 && ec[0x0727] == 0xa5);
            assert(uw_set_fan_auto() == 0);
            uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
            assert(ec[0x0727] == 0xe5);
        }
    } else if (!strcmp(test, "resume_failures")) {
        const int errors[] = {-EIO, -ETIMEDOUT, -ENODEV};
        for (unsigned int e = 0; e < ARRAY_SIZE(errors); ++e) {
            for (int point = 0; point < 4; ++point) {
                begin(false); manual(); assert(uniwill_keyboard_suspend(&platform, 0) == 0);
                fault_once(point & 1 ? 'w' : 'r', point < 2 ? 0x0727 : 0x078c, errors[e]);
                assert(uniwill_keyboard_resume(&platform) == errors[e]);
                assert(direct_fan_control_suspend && !direct_fan_control_restart_delayed_work.pending);
                assert(direct_fan_control_current_value_fan0_suspend_save == 83);
                assert(direct_fan_control_current_value_fan1_suspend_save == 97);
                assert(uw_set_fan(0, 110) == 0); run_worker();
                assert(ec[0x1804] == 110 && ec[0x1809] == 97);
                assert(uniwill_keyboard_resume(&platform) == 0 && !direct_fan_control_suspend);
            }
        }
    } else if (!strcmp(test, "resume_explicit_recovery")) {
        manual(); assert(uniwill_keyboard_suspend(&platform, 0) == 0);
        fault_once('w', 0x0727, -EIO); assert(uniwill_keyboard_resume(&platform) == -EIO);
        clear_events(); uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
        assert(ec[0x0727] == 0xe5 && find_event('L', 0) >= 0 && find_event('C', 0) >= 0);
        assert(uw_set_fan_auto() == 0 && !direct_fan_control_suspend);
        assert(!direct_fan_control_restart_delayed_work.pending);
    } else if (!strcmp(test, "adapter_remove")) {
        pause_kind = 'L'; pause_address = 0;
        interleave(ADAPTER, REMOVE);
        assert_serial_actors();
        int before = event_count; uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
        assert(event_count == before);
        begin(false); pause_kind = 'w'; pause_address = 0x0741;
        interleave(REMOVE, ADAPTER); assert(actor_events(2) == 0);
    } else if (!strcmp(test, "failed_suspend")) {
        manual(); fault_once('r', 0x0727, -EIO);
        assert(uniwill_keyboard_suspend(&platform, 0) == -EIO);
        clear_events(); uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
        assert(find_event('L', 0) >= 0 && ec[0x0727] == 0xe5);
        assert(uw_set_fan(0, 110) == 0); run_worker();
        assert(ec[0x1804] == 110 && ec[0x1809] == 97);
    } else if (!strcmp(test, "adapter_resume")) {
        pause_kind = 'L'; pause_address = 0;
        interleave(ADAPTER, RESUME); assert_serial_actors();
        begin(false); pause_kind = 'r'; pause_address = 0x0727;
        interleave(RESUME, ADAPTER); assert_serial_actors();
        assert(ec[0x0727] == 0xe5);
    } else if (!strcmp(test, "prepare_remove_worker")) {
        manual(); assert(direct_fan_control_restart_delayed_work.pending);
        uniwill_keyboard_prepare_remove(); clear_events(); run_worker();
        assert(event_count == 0 && !direct_fan_control_restart_delayed_work.pending);
        assert((int)uw_set_fan(0, 110) == -ENODEV && uw_set_mode(0x55) == -ENODEV);
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE); assert(event_count == 0);
        uniwill_keyboard_remove(&platform); begin(false); manual();
        assert(ec[0x1804] == 83 && ec[0x1809] == 97);
    } else if (!strcmp(test, "other_callbacks")) {
        static struct input_dev input;
        uniwill_keyboard_driver.input_device = &input;
        assert(uniwill_keyboard_suspend(&platform, 0) == 0); clear_events();
        uniwill_event_callb(UNIWILL_OSD_MODE_CHANGE_KEY_EVENT); assert(input_events == 8);
        uniwill_event_callb(UNIWILL_KEY_KBDILLUMTOGGLE); assert(input_events == 9);
        uniwill_event_callb(0xffff); assert(input_events == 10);
        uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
        assert(input_events == 10 && event_count == 0);
    } else return 2;
    return 0;
}
