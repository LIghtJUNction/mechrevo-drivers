/* Real driver functions, mocked EC/workqueue, deterministic pthread barriers. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#define TUXEDO_ERROR(format, status) record_error(format, status)
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
};
static struct uniwill_device_features_t uniwill_device_features;
static __attribute__((unused)) struct uniwill_device_features_t *uniwill_get_device_features(void) { return &uniwill_device_features; }
/* @LEGACY_PM_DECL@ */
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
static void uniwill_keyboard_prepare_remove(void) {}
static void uniwill_leds_remove(struct platform_device *dev);
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
static int uniwill_write_kbd_bl_enable(u8 enabled) { (void)enabled; return 0; }
static void uniwill_leds_restore_state_extern(void) {}
static void uw_charging_priority_write_state(void) {}
static void uw_charging_profile_write_state(void) {}
static int __attribute__((unused)) uniwill_i8042_filter;
static int __attribute__((unused)) i8042_remove_filter(int filter) { (void)filter; return 0; }
#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define EXPORT_SYMBOL(name)
#define pr_debug(...) do {} while (0)
#define pr_err(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define lockdep_assert_held(lock) do {} while (0)

static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static _Thread_local int actor;
static bool complete[4], waiting[4];
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
static int ec_accesses, fault_from = -1, fault_errno = -EIO, cleanup_mismatch_reads;
static bool faults_ram_only;
static int error_count, error_status[8];
static const char *error_format[8];
static void record_error(const char *format, int status)
{
    assert(error_count < 8);
    error_format[error_count] = format; error_status[error_count++] = status;
}
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
    *value = ec[address];
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, 'r', address, *value};
    int status = fault_from >= 0 && ec_accesses >= fault_from && (!faults_ram_only || address == 0x07c5 || address == 0x07c6) ? fault_errno : 0;
    ++ec_accesses;
    if (!status && address == 0x07c6 && !(ec[address] & 4) && cleanup_mismatch_reads) {
        --cleanup_mismatch_reads; *value ^= 4;
    }
    pause_if_selected('r', address);
    pthread_mutex_unlock(&harness);
    return status;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
    pthread_mutex_lock(&harness);
    /* Pause before committing, so competing requests expose lost updates. */
    pause_if_selected('w', address);
    assert(event_count < 2048);
    events[event_count++] = (struct event){actor, 'w', address, value};
    int status = fault_from >= 0 && ec_accesses >= fault_from && (!faults_ram_only || address == 0x07c5 || address == 0x07c6) ? fault_errno : 0;
    ++ec_accesses;
    if (!status) ec[address] = value;
    if (address >= 0x0f00 && address <= 0x0f5f)
        ++table_writes;
    pthread_mutex_unlock(&harness);
    return status;
}

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop

/* Baseline lacks the two serialized mode exports and reprobe helper. */
/* @BASELINE_COMPAT@ */

enum operation { MANUAL, AUTO, INIT, FULL, WORKER, SUSPEND, RESUME, REMOVE, UPDATE };
static enum operation operations[4];
static void *run(void *arg)
{
    actor = (int)(intptr_t)arg;
    switch (operations[actor]) {
    case MANUAL: result[actor] = (int)uw_set_fan(0, 110); break;
    case AUTO: result[actor] = (int)uw_set_fan_auto(); break;
    case INIT: result[actor] = uw_init_fan(); break;
    case FULL: result[actor] = set_full_fan_mode(true); break;
    case UPDATE: result[actor] = uw_update_mode(0xb0, 0xa0); break;
    case SUSPEND: result[actor] = uniwill_keyboard_suspend(NULL, 0); break;
    case RESUME: result[actor] = uniwill_keyboard_resume(NULL); break;
    case REMOVE: uniwill_keyboard_remove(NULL); result[actor] = 0; break;
    case WORKER:
        pthread_mutex_lock(&harness);
        assert(direct_fan_control_restart_delayed_work.pending);
        direct_fan_control_restart_delayed_work.pending = false;
        direct_fan_control_restart_delayed_work.running = true;
        pthread_mutex_unlock(&harness);
        restart_direct_fan_control_work_handler(&direct_fan_control_restart_delayed_work.work);
        pthread_mutex_lock(&harness);
        direct_fan_control_restart_delayed_work.running = false;
        pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&harness);
        break;
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
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0);
    end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!*condition)
        assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    pthread_mutex_unlock(&harness);
}
static void wait_contender(void)
{
    struct timespec end;
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0);
    end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!complete[2] && !waiting[2])
        assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    release_pause = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
}
static void interleave(enum operation first, enum operation second)
{
    pthread_t one = start(1, first);
    wait_until(&paused);
    pthread_t two = start(2, second);
    wait_contender();
    assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
    assert(result[1] == 0);
    assert(result[2] == (first == SUSPEND ? -EBUSY : 0));
}
static void prepare_manual(void)
{
    direct_fan_control_started = true;
    direct_fan_control_current_value_fan0 = 83;
    direct_fan_control_current_value_fan1 = 97;
    direct_fan_control_restart_delayed_work.pending = true;
    ec[0x0751] = 0x51;
    ec[0x1804] = 83; ec[0x1809] = 97;
}
static void prepare_universal(void)
{
    uniwill_device_features.uniwill_has_universal_ec_fan_control = true;
    ec[0x0751] = 0x11;
    assert(uw_set_fan(0, 83) == 0 && uw_set_fan(1, 97) == 0);
    assert(universal_fan_manual_active && !direct_fan_control_started);
    assert(!direct_fan_control_restart_delayed_work.pending);
    event_count = table_writes = 0;
}
static void reset_removed_lifecycle(void)
{
    fault_from = -1; faults_ram_only = false; cleanup_mismatch_reads = 0;
    assert(uw_fan_control_start() == 0);
    uniwill_pm_ready = true;
    prepare_universal();
    ec_accesses = error_count = event_count = 0;
}
static void assert_remove_order(void)
{
    int last_ram = -1, resource = -1, ap = -1;
    for (int i = 0; i < event_count; ++i) {
        if (events[i].address == 0x07c5 || events[i].address == 0x07c6) last_ram = i;
        if (events[i].kind == 'L') resource = i;
        if (events[i].kind == 'w' && events[i].address == 0x0741) ap = i;
    }
    assert(resource >= 0 && ap > resource && resource > last_ram);
    assert(uniwill_fan_removed && uniwill_fan_work_paused);
    assert(!direct_fan_control_suspend && !fans_initialized && !direct_fan_control_restart_delayed_work.pending);
}
static void assert_auto(void)
{
    assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
    assert(!direct_fan_control_suspend);
    assert(direct_fan_control_current_value_fan0 == 0 && direct_fan_control_current_value_fan1 == 0);
    assert((ec[0x0751] & 0x40) == 0);
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *test = argv[1];
    actor = 0;
    /* Fan-only schedules start after probe; the PM suite runs the real probe. */
    uniwill_pm_ready = true;
    if (strcmp(test, "auto_manual") == 0) {
        prepare_manual(); pause_address = 0x0751;
        interleave(AUTO, MANUAL);
        assert(direct_fan_control_current_value_fan0 == 110 && direct_fan_control_started);
        assert(direct_fan_control_restart_delayed_work.pending);
    } else if (strcmp(test, "worker_manual") == 0) {
        prepare_manual(); pause_address = 0x1804; pause_occurrence = 10;
        interleave(WORKER, MANUAL);
        assert(ec[0x1804] == 110 && direct_fan_control_current_value_fan0 == 110);
    } else if (strcmp(test, "worker_auto") == 0) {
        prepare_manual(); pause_address = 0x1804;
        interleave(WORKER, AUTO);
        assert_auto();
    } else if (strcmp(test, "init_auto") == 0) {
        uniwill_device_features.uniwill_has_universal_ec_fan_control = true;
        pause_address = 0x07c6;
        interleave(INIT, AUTO);
        assert((ec[0x07c5] & 0x80) == 0 && (ec[0x07c6] & 4) == 0 && !fans_initialized);
    } else if (strcmp(test, "init_init") == 0) {
        uniwill_device_features.uniwill_has_universal_ec_fan_control = true;
        pause_address = 0x0f00;
        interleave(INIT, INIT);
        assert(table_writes == 96 && fans_initialized);
    } else if (strcmp(test, "full_auto") == 0) {
        ec[0x0751] = 0x11; pause_address = 0x0751;
        interleave(FULL, AUTO);
        assert_auto();
    } else if (strcmp(test, "suspend_manual") == 0) {
        prepare_manual(); pause_address = 0x0751;
        interleave(SUSPEND, MANUAL);
        assert(uniwill_pm_suspended);
        assert(direct_fan_control_current_value_fan0_suspend_save == 83);
        assert(direct_fan_control_current_value_fan1_suspend_save == 97);
        assert(uniwill_keyboard_resume(NULL) == 0);
        assert(!uniwill_pm_suspended);
        assert(uw_set_fan(0, 110) == 0);
        assert(direct_fan_control_current_value_fan0 == 110 && direct_fan_control_current_value_fan1 == 97);
    } else if (strcmp(test, "resume_auto") == 0) {
        prepare_manual(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        pause_address = 0x1809;
        interleave(RESUME, AUTO);
        assert_auto();
    } else if (strcmp(test, "worker_remove") == 0) {
        prepare_manual(); pause_address = 0x1804;
        interleave(WORKER, REMOVE);
        int disable = -1, leds_removed = -1, last_fan_write = -1;
        for (int i = 0; i < event_count; ++i) {
            if (events[i].kind == 'w' && events[i].address == 0x0741) disable = i;
            if (events[i].kind == 'L') leds_removed = i;
            if (events[i].kind == 'w' && (events[i].address == 0x1804 || events[i].address == 0x1809)) last_fan_write = i;
        }
        assert(leds_removed > last_fan_write && disable > leds_removed);
        assert(!direct_fan_control_restart_delayed_work.pending);
    } else if (strcmp(test, "remove_manual") == 0) {
        pause_address = 0x0741;
        pthread_t one = start(1, REMOVE); wait_until(&paused);
        pthread_t two = start(2, MANUAL); wait_contender();
        assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
        assert(result[2] == -ENODEV && !direct_fan_control_restart_delayed_work.pending);
        int before = event_count;
        assert((int)uw_set_fan(0, 110) == -ENODEV);
        assert((int)uw_set_fan_auto() == -ENODEV && uw_init_fan() == -ENODEV);
        assert(set_full_fan_mode(true) == -ENODEV);
        assert(uw_set_mode(0x55) == -ENODEV && uw_update_mode(0xb0, 0xa0) == -ENODEV);
        assert(event_count == before);
    } else if (strcmp(test, "universal_suspend_manual") == 0) {
        prepare_universal(); pause_address = 0x07c6; pause_kind = 'r';
        interleave(SUSPEND, MANUAL);
        assert(uniwill_pm_suspended && direct_fan_control_suspend && !universal_fan_manual_active);
        assert(direct_fan_control_current_value_fan0_suspend_save == 83);
        assert(direct_fan_control_current_value_fan1_suspend_save == 97);
        assert(!(ec[0x07c6] & 4) && !direct_fan_control_restart_delayed_work.pending);
    } else if (strcmp(test, "universal_resume_auto") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        memset(ec + 0x0f00, 0, 0x60);
        pause_address = 0x0f20; pause_kind = 'w';
        interleave(RESUME, AUTO);
        assert_auto(); assert(!universal_fan_manual_active && !(ec[0x07c6] & 4));
    } else if (strcmp(test, "universal_resume_remove") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        memset(ec + 0x0f00, 0, 0x60);
        pause_address = 0x0f20; pause_kind = 'w';
        interleave(RESUME, REMOVE);
        assert(uniwill_fan_removed && !direct_fan_control_restart_delayed_work.pending);
        int before = event_count;
        assert((int)uw_set_fan(0, 110) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
        assert(uniwill_keyboard_resume(NULL) == -ENODEV && event_count == before);
    } else if (strcmp(test, "universal_remove_owned") == 0) {
        prepare_universal(); assert(set_full_fan_mode(true) == 0);
        ec[0x07c5] |= 0x23; ec[0x07c6] |= 0xa3;
        event_count = ec_accesses = 0;
        uniwill_keyboard_remove(NULL);
        assert_remove_order();
        assert(!universal_fan_manual_active && error_count == 0);
        assert(ec[0x07c5] == 0x23 && ec[0x07c6] == 0xa3 && ec[0x0751] == 0x51);
        int before = event_count;
        assert((int)uw_set_fan(0, 110) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
        assert(uniwill_keyboard_resume(NULL) == -ENODEV && event_count == before);
    } else if (strcmp(test, "universal_remove_failures") == 0) {
        reset_removed_lifecycle(); uniwill_keyboard_remove(NULL);
        int count = ec_accesses;
        assert(count == 7); /* six AUTO accesses then mandatory AP-disable */
        const int codes[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (unsigned int code = 0; code < sizeof(codes)/sizeof(codes[0]); ++code) {
            for (int failure = 0; failure < count; ++failure) {
                reset_removed_lifecycle(); assert(set_full_fan_mode(true) == 0);
                event_count = ec_accesses = 0;
                fault_errno = codes[code]; fault_from = failure;
                uniwill_keyboard_remove(NULL); assert_remove_order();
                assert(ec[0x0751] == 0x51);
                if (failure < count-1) {
                    assert(universal_fan_manual_active && error_count == 2);
                    assert(strstr(error_format[0], "automatic fan control during removal"));
                    assert(error_status[0] == codes[code]);
                } else assert(!universal_fan_manual_active && error_count == 1);
                assert(strstr(error_format[error_count-1], "disable manual fan mode"));
                assert(error_status[error_count-1] == codes[code]);
                int before = event_count;
                assert((int)uw_set_fan(0, 110) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
                assert(event_count == before);
            }
        }
    } else if (strcmp(test, "universal_remove_cleanup_only_failure") == 0) {
        const int codes[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (unsigned int code = 0; code < sizeof(codes)/sizeof(codes[0]); ++code) {
            for (int failure = 0; failure < 6; ++failure) {
                reset_removed_lifecycle(); ec[0x0741] = 1;
                fault_errno = codes[code]; faults_ram_only = true; fault_from = failure;
                uniwill_keyboard_remove(NULL); assert_remove_order();
                assert(universal_fan_manual_active && error_count == 1);
                assert(strstr(error_format[0], "automatic fan control during removal") && error_status[0] == codes[code]);
                assert(ec[0x0741] == 0); /* mandatory teardown succeeds despite cleanup failure */
            }
        }
        reset_removed_lifecycle(); cleanup_mismatch_reads = 3; ec[0x0741] = 1;
        uniwill_keyboard_remove(NULL); assert_remove_order();
        assert(cleanup_mismatch_reads == 0 && error_count == 1 && error_status[0] == -EIO);
        assert(universal_fan_manual_active && !(ec[0x07c6] & 4)); /* disabled storage, unverified readback */
        assert(ec[0x0741] == 0);
    } else if (strcmp(test, "universal_remove_reprobe") == 0) {
        /* Failed removal must not lose the RAM owner on a same-module rebind. */
        reset_removed_lifecycle(); faults_ram_only = true; fault_from = 0;
        uniwill_keyboard_remove(NULL);
        assert(universal_fan_manual_active && (ec[0x07c6] & 4) && ec[0x0741] == 0);
        fault_from = -1; faults_ram_only = false; event_count = ec_accesses = error_count = 0;
        assert(uw_fan_control_start() == 0);
        assert(!universal_fan_manual_active && !(ec[0x07c6] & 4) && !(ec[0x07c5] & 0x80));
        assert(!uniwill_pm_ready && !uniwill_fan_removed && !direct_fan_control_suspend && !fans_initialized);
        int count = ec_accesses; assert(count == 7);
        for (int failure = 0; failure < count; ++failure) {
            reset_removed_lifecycle(); faults_ram_only = true; fault_from = 0;
            uniwill_keyboard_remove(NULL);
            fault_from = failure; faults_ram_only = false; event_count = ec_accesses = error_count = 0;
            assert(uw_fan_control_start() == -EIO);
            assert(uniwill_fan_removed && !uniwill_pm_ready && !direct_fan_control_suspend && !fans_initialized);
            assert(universal_fan_manual_active == (failure < count-1));
            int before = event_count;
            assert((int)uw_set_fan(0, 110) == -ENODEV && uw_init_fan() == -ENODEV);
            assert((int)uw_set_fan_auto() == -ENODEV && event_count == before);
            fault_from = -1; event_count = ec_accesses = 0;
            assert(uw_fan_control_start() == 0 && !universal_fan_manual_active);
            assert(!(ec[0x07c6] & 4) && !uniwill_pm_ready && !uniwill_fan_removed);
        }
        reset_removed_lifecycle(); faults_ram_only = true; fault_from = 0;
        uniwill_keyboard_remove(NULL);
        fault_from = -1; faults_ram_only = false;
        uniwill_device_features.uniwill_has_universal_ec_fan_control = false;
        event_count = ec_accesses = 0;
        assert(uw_fan_control_start() == -ENODEV && universal_fan_manual_active);
        assert(uniwill_fan_removed && !uniwill_pm_ready && ec_accesses == 0);
        uniwill_device_features.uniwill_has_universal_ec_fan_control = true;
        assert(uw_fan_control_start() == 0 && !universal_fan_manual_active);
    } else if (strcmp(test, "universal_remove_unowned") == 0) {
        for (int branch = 0; branch < 3; ++branch) {
            fault_from = -1; assert(uw_fan_control_start() == 0); uniwill_pm_ready = true;
            uniwill_device_features.uniwill_has_universal_ec_fan_control = branch != 2;
            universal_fan_manual_active = branch == 2; /* impossible flag/cap mismatch must not touch RAM */
            if (branch == 1) {
                prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
                assert(direct_fan_control_suspend && !universal_fan_manual_active);
            }
            ec[0x07c5] = 0xa3; ec[0x07c6] = 0xa7; /* external/unowned table */
            event_count = ec_accesses = error_count = 0;
            uniwill_keyboard_remove(NULL); assert_remove_order();
            assert(ec_accesses == 1 && error_count == 0);
            assert(ec[0x07c5] == 0xa3 && ec[0x07c6] == 0xa7);
        }
    } else if (strcmp(test, "universal_remove_manual") == 0) {
        prepare_universal(); pause_address = 0x07c6; pause_kind = 'r';
        pthread_t one = start(1, REMOVE); wait_until(&paused);
        pthread_t two = start(2, MANUAL); wait_contender();
        assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
        assert(result[2] == -ENODEV && !universal_fan_manual_active && !(ec[0x07c6] & 4));
        assert_remove_order();
    } else if (strcmp(test, "mode_update") == 0) {
        ec[0x0751] = 0x15; pause_address = 0x0751; pause_kind = 'r';
        interleave(FULL, UPDATE);
        assert(ec[0x0751] == 0xe5);
    } else if (strcmp(test, "reprobe") == 0) {
        prepare_manual(); uniwill_keyboard_remove(NULL);
        assert(uw_fan_control_start() == 0);
        assert(!direct_fan_control_started && !fans_initialized);
        assert(!direct_fan_control_restart_delayed_work.pending);
        assert(!uniwill_pm_ready && !uniwill_pm_suspended);
        int before = event_count;
        assert(uw_set_mode(0x11) == -ENODEV && (int)uw_set_fan(0, 110) == -ENODEV);
        assert((int)uw_set_fan_auto() == -ENODEV && uw_init_fan() == -ENODEV);
        assert(set_full_fan_mode(true) == -ENODEV && uw_update_mode(0xb0, 0xa0) == -ENODEV);
        assert(event_count == before && direct_fan_control_current_value_fan0 == 0);
    } else return 2;
    return 0;
}
