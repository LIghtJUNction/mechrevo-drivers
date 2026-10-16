/* Real common/Uniwill remove, i8042 filter and deferred key worker; mocked kernel. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#define TUXEDO_ERROR(...) do {} while (0)
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
typedef uint8_t u8;
typedef uint32_t u32;
struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(n) struct mutex n = { PTHREAD_MUTEX_INITIALIZER }
static void note_mutex_wait(void);
static void mutex_lock(struct mutex *m) {
    int status = pthread_mutex_trylock(&m->native);
    if (status == EBUSY) { note_mutex_wait(); status = pthread_mutex_lock(&m->native); }
    assert(status == 0);
}
static void mutex_unlock(struct mutex *m) { assert(pthread_mutex_unlock(&m->native) == 0); }
static DEFINE_MUTEX(tuxedo_keyboard_init_driver_lock);
static DEFINE_MUTEX(uniwill_fan_command_lock);
static DEFINE_MUTEX(uniwill_fan_state_lock);
struct work_struct { bool pending, running; };
static struct work_struct uniwill_key_event_work;
struct input_dev { bool freed; };
struct device { struct { int unused; } kobj; };
struct platform_device { struct device dev; };
struct platform_driver { struct { const char *name; } driver; };
struct tuxedo_keyboard_driver { struct input_dev *input_device; struct platform_driver *platform_driver; void (*prepare_remove)(void); };
static void uniwill_keyboard_prepare_remove(void);
struct serio { int unused; };
typedef bool (*filter_fn)(unsigned char, unsigned char, struct serio *, void *);
static struct input_dev input;
static struct platform_device platform;
static struct platform_driver driver = { { "tuxedo_keyboard" } }, other = { { "tuxedo_keyboard" } };
static struct tuxedo_keyboard_driver uniwill_keyboard_driver = { &input, &driver, /* @PREPARE_REMOVE_HOOK@ */ };
static struct tuxedo_keyboard_driver other_driver = { &input, &other, NULL };
static struct tuxedo_keyboard_driver *current_driver = &uniwill_keyboard_driver;
static struct platform_device *tuxedo_platform_device = &platform;
static struct input_dev *tuxedo_input_device = &input;
static bool uw_charging_prio_loaded, uw_charging_profile_loaded, uw_lightbar_loaded;
static int uw_charging_prio_attr_group, uw_charging_profile_attr_group, dev_attr_fn_lock;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
static bool uniwill_fan_removed;
static bool uniwill_pm_ready, uniwill_pm_suspended;
#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a,b,c) (((a)<<16)|((b)<<8)|(c))
#define EXPORT_SYMBOL(n)
#define TUXEDO_DEBUG(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define IS_ERR_OR_NULL(p) (!(p) || (uintptr_t)(p) >= (uintptr_t)-4095)
#define unlikely(v) (v)
#define ARRAY_SIZE(a) (sizeof(a)/sizeof((a)[0]))
#define I8042_STR_AUXDATA 0x20
#define __always_unused __attribute__((unused))
#define UNIWILL_OSD_TOUCHPADWORKAROUND 0xfff
static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static bool uniwill_i8042_filter_installed = true;
static bool filter_installed = true, entered_sleep, release_sleep, draining, removed;
static bool pause_worker, deliver_late, input_was_absent, direct_mode;
static int clock_event, filter_stop_order, drain_order, input_free_order, reports, frees, drains;
static bool parallel_case, remove_filter_paused, release_filter, helper_two_done, helper_two_waiting;
static int filter_removals;
static _Thread_local int helper_actor;
static void note_mutex_wait(void) {
    if (!parallel_case || helper_actor != 2) return;
    pthread_mutex_lock(&harness); helper_two_waiting = true;
    pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
}
static void key_event_work(struct work_struct *work);
static void uniwill_keyboard_remove(struct platform_device *dev);
static bool uniwill_i8042_filter(unsigned char data, unsigned char str, struct serio *port, void *context);
static void send_sequence(void);
static bool schedule_work(struct work_struct *work) {
    pthread_mutex_lock(&harness);
    bool queued = !work->pending; work->pending = true;
    pthread_mutex_unlock(&harness); return queued;
}
static __attribute__((unused)) void cancel_work_sync(struct work_struct *work) {
    pthread_mutex_lock(&harness);
    ++drains; draining = true; pthread_cond_broadcast(&changed);
    while (work->running) pthread_cond_wait(&changed, &harness);
    work->pending = false;
    if (!drain_order) drain_order = ++clock_event;
    else { assert(!work->running && !work->pending); ++clock_event; }
    pthread_mutex_unlock(&harness);
}
static void msleep(unsigned int value) {
    assert(value == 50);
    pthread_mutex_lock(&harness);
    entered_sleep = true; pthread_cond_broadcast(&changed);
    while (pause_worker && !release_sleep) pthread_cond_wait(&changed, &harness);
    pthread_mutex_unlock(&harness);
}
static int i8042_remove_filter(filter_fn fn) {
    assert(fn == uniwill_i8042_filter);
    pthread_mutex_lock(&harness);
    bool was_installed = filter_installed;
    ++filter_removals;
    filter_installed = false; filter_stop_order = ++clock_event;
    if (parallel_case && helper_actor == 1) {
        remove_filter_paused = true; pthread_cond_broadcast(&changed);
        while (!release_filter) pthread_cond_wait(&changed, &harness);
    }
    pthread_mutex_unlock(&harness);
    if (deliver_late) send_sequence();
    return was_installed ? 0 : -EINVAL;
}
static bool sparse_keymap_report_known_event(struct input_dev *dev, u32 code, int value, bool repeat) {
    assert(dev == &input && !input.freed); assert(code == UNIWILL_OSD_TOUCHPADWORKAROUND);
    assert(value == 1 && repeat); ++reports; return true;
}
static void sysfs_remove_group(void *kobj, void *group) { (void)kobj; (void)group; }
/* Charging lifecycle is exercised with its real implementation separately. */
static __attribute__((unused)) void uw_charging_uninit(struct platform_device *dev)
{
    (void)dev;
    if (uw_charging_prio_loaded) sysfs_remove_group(NULL, &uw_charging_prio_attr_group);
    if (uw_charging_profile_loaded) sysfs_remove_group(NULL, &uw_charging_profile_attr_group);
}
static void uw_battery_uninit(void) {}
static void uniwill_leds_remove(struct platform_device *dev) { assert(dev == &platform); }
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
static int uniwill_write_kbd_bl_enable(u8 b) { (void)b; return 0; }
static int uniwill_write_ec_ram(unsigned int address, u8 b) { assert(address == 0x0741 && b == 0); return 0; }
static void uw_fan_quiesce_locked(void) {}
static void device_remove_file(struct device *dev, int *attr) { assert(dev == &platform.dev); (void)attr; }
static void input_unregister_device(struct input_dev *dev) {
    pthread_mutex_lock(&harness);
    assert(dev == &input && !input.freed);
    assert(!filter_installed && !uniwill_key_event_work.running && !uniwill_key_event_work.pending);
    input.freed = true; ++frees; input_free_order = ++clock_event;
    pthread_mutex_unlock(&harness);
}
static void platform_device_unregister(struct platform_device *dev) { uniwill_keyboard_remove(dev); }
static void platform_driver_unregister(struct platform_driver *drv) { assert(drv == &driver); }
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
static struct { bool uniwill_has_universal_ec_fan_control; } uniwill_device_features;
static bool universal_fan_manual_active, direct_fan_control_suspend, fans_initialized;
/* This key-work-only fixture never starts RAM fan control. */
static int uw_set_fan_auto_locked(void) {
    assert(!"Unexpected owned RAM table in key-work-only fixture"); return 0;
}
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop
static void send_sequence(void) {
    pthread_mutex_lock(&harness); bool active = filter_installed; pthread_mutex_unlock(&harness);
    if (!active) return;
    for (unsigned int i = 0; i < ARRAY_SIZE(uniwill_touchp_toggle_seq); ++i)
        (void)uniwill_i8042_filter(uniwill_touchp_toggle_seq[i], 0, NULL, NULL);
}
static void *run_worker(void *unused) {
    (void)unused;
    pthread_mutex_lock(&harness);
    assert(uniwill_key_event_work.pending); uniwill_key_event_work.pending = false;
    uniwill_key_event_work.running = true;
    pthread_mutex_unlock(&harness);
    key_event_work(&uniwill_key_event_work);
    pthread_mutex_lock(&harness);
    uniwill_key_event_work.running = false; pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness); return NULL;
}
static void *remove_driver(void *unused) {
    (void)unused;
    if (direct_mode) uniwill_keyboard_remove(&platform);
    else tuxedo_keyboard_remove_driver(&uniwill_keyboard_driver);
    pthread_mutex_lock(&harness); removed = true; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
    return NULL;
}
static void *prepare_events(void *arg) {
    helper_actor = (int)(intptr_t)arg; uniwill_keyboard_prepare_remove();
    if (helper_actor == 2) {
        pthread_mutex_lock(&harness); helper_two_done = true;
        pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
    }
    return NULL;
}
static void wait_flag(bool *flag) {
    struct timespec deadline; assert(clock_gettime(CLOCK_REALTIME, &deadline) == 0); deadline.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!*flag && !(flag == &draining && removed) && !(flag == &helper_two_waiting && helper_two_done)) assert(pthread_cond_timedwait(&changed, &harness, &deadline) == 0);
    pthread_mutex_unlock(&harness);
}
int main(int argc, char **argv) {
    assert(argc == 2); const char *test = argv[1];
    direct_mode = !strcmp(test, "direct_queued") || !strcmp(test, "direct_running");
    input_was_absent = !strcmp(test, "no_input_queued") || !strcmp(test, "no_input_running");
    if (input_was_absent) { tuxedo_input_device = NULL; uniwill_keyboard_driver.input_device = NULL; }
    if (!strcmp(test, "filter_absent")) { filter_installed = false; uniwill_i8042_filter_installed = false; uniwill_key_event_work.pending = true; }
    else send_sequence();
    if (!strcmp(test, "parallel_prepare")) {
        parallel_case = true; pthread_t one, two;
        assert(pthread_create(&one, NULL, prepare_events, (void *)(intptr_t)1) == 0);
        wait_flag(&remove_filter_paused);
        assert(pthread_create(&two, NULL, prepare_events, (void *)(intptr_t)2) == 0);
        wait_flag(&helper_two_waiting);
        pthread_mutex_lock(&harness); release_filter = true;
        pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
        assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
        assert(filter_removals == 1 && drains == 2 && !uniwill_key_event_work.pending);
        assert(!input.freed && uniwill_keyboard_driver.input_device == &input); return 0;
    }
    if (!strcmp(test, "wrong_driver")) {
        tuxedo_keyboard_remove_driver(&other_driver);
        assert(filter_installed && uniwill_key_event_work.pending && current_driver == &uniwill_keyboard_driver && drains == 0);
        return 0;
    }
    bool running = !strcmp(test, "running") || !strcmp(test, "no_input_running") || !strcmp(test, "direct_running");
    if (!strcmp(test, "late_event")) deliver_late = true;
    if (running) {
        pause_worker = true; pthread_t worker, remover;
        assert(pthread_create(&worker, NULL, run_worker, NULL) == 0); wait_flag(&entered_sleep);
        assert(pthread_create(&remover, NULL, remove_driver, NULL) == 0); wait_flag(&draining);
        pthread_mutex_lock(&harness);
        bool completed_before_drain = removed;
        if (!completed_before_drain) {
            assert(!input.freed && uniwill_keyboard_driver.input_device == (input_was_absent ? NULL : &input));
            assert(!filter_installed);
        }
        release_sleep = true; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
        assert(pthread_join(worker, NULL) == 0 && pthread_join(remover, NULL) == 0);
        assert(!completed_before_drain);
    } else if (direct_mode) uniwill_keyboard_remove(&platform);
    else tuxedo_keyboard_remove_driver(&uniwill_keyboard_driver);
    assert(!filter_installed && !uniwill_key_event_work.pending && !uniwill_key_event_work.running);
    assert(drain_order > filter_stop_order && drains >= 1);
    if (strcmp(test, "filter_absent")) assert(filter_stop_order > 0);
    if (direct_mode) {
        assert(frees == 0 && !input.freed && uniwill_fan_removed);
        assert(current_driver == &uniwill_keyboard_driver && tuxedo_platform_device == &platform);
        assert(tuxedo_input_device == &input && uniwill_keyboard_driver.input_device == &input);
        assert(reports == (running ? 1 : 0)); return 0;
    }
    assert(frees == (input_was_absent ? 0 : 1));
    if (!input_was_absent) assert(input_free_order > drain_order);
    assert(reports == (running && !input_was_absent ? 1 : 0));
    assert(!current_driver && !tuxedo_platform_device && !tuxedo_input_device && !uniwill_keyboard_driver.input_device);
    if (!strcmp(test, "repeated")) {
        int previous_drains = drains;
        tuxedo_keyboard_remove_driver(&uniwill_keyboard_driver);
        tuxedo_keyboard_remove_driver(NULL); assert(drains == previous_drains && frees == 1);
    }
    return 0;
}
