/* Actual production IO/provider/PM functions; EC/work services are userspace mocks. */
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
    bool uniwill_has_double_pl4;
    bool uniwill_profile_v1_two_profs;
    bool uniwill_profile_v1_three_profs;
    bool uniwill_profile_v1_three_profs_leds_only;
};
static struct uniwill_device_features_t uniwill_device_features;
static struct uniwill_device_features_t *uw_feats = &uniwill_device_features;
static __attribute__((unused)) struct uniwill_device_features_t *uniwill_get_device_features(void)
{ return &uniwill_device_features; }
static const int limits_min[3] = {10, 10, 10}, limits_max[3] = {200, 200, 200};
static const int *tdp_min_defs = limits_min, *tdp_max_defs = limits_max;

struct platform_device { struct { struct { int unused; } kobj; } dev; };
typedef int pm_message_t;
struct work_struct { int unused; };
struct delayed_work { struct work_struct work; bool pending, running; void (*handler)(struct work_struct *); };
static struct work_struct uniwill_key_event_work;
static bool uniwill_i8042_filter_installed;
static unsigned int uniwill_touchp_toggle_seq_pos;
static bool uw_charging_prio_loaded, uw_charging_profile_loaded, uw_lightbar_loaded;
static int uw_charging_prio_attr_group, uw_charging_profile_attr_group;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
static int uniwill_i8042_filter;
static int i8042_remove_filter(int filter) { (void)filter; return 0; }
static void cancel_work_sync(struct work_struct *work) { (void)work; }
static void sysfs_remove_group(void *kobj, void *group) { (void)kobj; (void)group; }
static void uw_battery_uninit(void) {}
static void uniwill_leds_remove(struct platform_device *dev) { (void)dev; }
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
static int uniwill_write_kbd_bl_enable(u8 enabled) { (void)enabled; return 0; }
static void uniwill_leds_restore_state_extern(void) {}
static void uw_charging_priority_write_state(void) {}
static void uw_charging_profile_write_state(void) {}
#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define EXPORT_SYMBOL(name)
#define TUXEDO_ERROR(...) do {} while (0)
#define pr_debug(...) do {} while (0)
#define pr_err(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define lockdep_assert_held(lock) do {} while (0)

static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static _Thread_local int actor;
static bool complete[3], waiting[3], waiting_on_command[3];
static int result[3];
static bool tdp_request_running[3], tdp_final_attempted[3];
static unsigned int premature_command_releases[3];
struct mutex { pthread_mutex_t native; const char *name; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER, #name }
static void mutex_lock(struct mutex *lock)
{
    int status = pthread_mutex_trylock(&lock->native);
    if (status == EBUSY) {
        pthread_mutex_lock(&harness);
        waiting[actor] = true;
        waiting_on_command[actor] = strcmp(lock->name, "uniwill_fan_command_lock") == 0;
        pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&harness);
        status = pthread_mutex_lock(&lock->native);
    }
    assert(status == 0);
}
static void mutex_unlock(struct mutex *lock)
{
    pthread_mutex_lock(&harness);
    /* Observe the real command lock boundary too: a helper that drops the
     * mode lock and reacquires it solely around the final write otherwise
     * looks serialized at our final-write pause. No source-pattern assertion
     * or copied production lock implementation substitutes for this event. */
    if (strcmp(lock->name, "uniwill_fan_command_lock") == 0 &&
        tdp_request_running[actor] && !tdp_final_attempted[actor])
        ++premature_command_releases[actor];
    pthread_mutex_unlock(&harness);
    assert(pthread_mutex_unlock(&lock->native) == 0);
}
#define DECLARE_DELAYED_WORK(name, fn) struct delayed_work name = {.handler = fn}
static bool schedule_delayed_work(struct delayed_work *work, unsigned int delay)
{
    (void)delay;
    bool queued = !work->pending;
    work->pending = true;
    return queued;
}
static void cancel_delayed_work_sync(struct delayed_work *work)
{
    /* These schedules do not run a fan worker. Its production code is still
     * compiled and its pending state is observable to the real PM helpers. */
    assert(!work->running);
    work->pending = false;
}
#define msecs_to_jiffies(value) (value)
static void msleep(unsigned int value) { (void)value; }

struct event {
    int actor;
    char kind;
    u16 address;
    u8 value, mode;
    int status;
    bool ready, suspended, removed;
};
static struct event events[2048];
static int event_count;
static u8 ec[UINT16_MAX + 1u];
static bool paused, release_pause;
static int failure_status, failure_occurrence = 1, failure_seen;
static char failure_kind;
static u16 failure_address;
static void snapshot_provider(struct event *event);

static int transport_status(char kind, u16 address)
{
    if (kind == failure_kind && address == failure_address &&
        ++failure_seen == failure_occurrence)
        return failure_status;
    return 0;
}
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
    pthread_mutex_lock(&harness);
    int status = transport_status('r', address);
    if (!status)
        *value = ec[address];
    assert(event_count < 2048);
    events[event_count++] = (struct event){.actor = actor, .kind = 'r',
        .address = address, .value = ec[address], .status = status};
    pthread_mutex_unlock(&harness);
    return status;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
    pthread_mutex_lock(&harness);
    /* Stop actor 1 immediately before the final device write, after the old
     * implementation has dropped its mode locks. A condition variable, not
     * a guessed sleep, determines when the contender has attempted entry. */
    if (actor == 1 && address == 0x0783 && !paused) {
        paused = true;
        pthread_cond_broadcast(&changed);
        while (!release_pause)
            pthread_cond_wait(&changed, &harness);
    }
    int status = transport_status('w', address);
    assert(event_count < 2048);
    struct event *event = &events[event_count++];
    *event = (struct event){.actor = actor, .kind = 'w', .address = address,
        .value = value, .mode = ec[0x0751], .status = status};
    if (address >= 0x0783 && address <= 0x0785) {
        tdp_final_attempted[actor] = true;
        snapshot_provider(event);
    }
    if (!status)
        ec[address] = value;
    pthread_mutex_unlock(&harness);
    return status;
}

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop
/* @HAS_TDP_PROVIDER@ */

static void snapshot_provider(struct event *event)
{
    event->ready = uniwill_pm_ready;
    event->suspended = uniwill_pm_suspended;
    event->removed = uniwill_fan_removed;
}

enum operation { TDP, PROFILE, RAW_MODE, FULL_FAN, SUSPEND, PREPARE_REMOVE, REMOVE };
static enum operation operations[3];
static void *run(void *arg)
{
    actor = (int)(intptr_t)arg;
    switch (operations[actor]) {
    case TDP:
        tdp_request_running[actor] = true;
        result[actor] = uw_set_tdp(0, actor == 1 ? 85 : 102);
        tdp_request_running[actor] = false;
        break;
    case PROFILE: result[actor] = uw_set_performance_profile_v1(PROFILE_POWERSAVE); break;
    case RAW_MODE: result[actor] = uw_set_mode(0x2f); break;
    case FULL_FAN: result[actor] = set_full_fan_mode(true); break;
    case SUSPEND: result[actor] = uniwill_keyboard_suspend(NULL, 0); break;
    case PREPARE_REMOVE: uniwill_keyboard_prepare_remove(); result[actor] = 0; break;
    case REMOVE: uniwill_keyboard_remove(NULL); result[actor] = 0; break;
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
static void wait_pause(void)
{
    struct timespec end;
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0);
    end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!paused)
        assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    pthread_mutex_unlock(&harness);
}
static bool release_after_contender(void)
{
    struct timespec end;
    assert(clock_gettime(CLOCK_REALTIME, &end) == 0);
    end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!complete[2] && !waiting[2])
        assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    bool contender_blocked = waiting[2] && waiting_on_command[2] && !complete[2];
    release_pause = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    return contender_blocked;
}
static void prepare(bool custom)
{
    actor = 0;
    uniwill_pm_ready = true;
    uniwill_pm_suspended = false;
    uniwill_fan_removed = false;
    uniwill_device_features.uniwill_custom_profile_mode_needed = custom;
    uniwill_device_features.uniwill_profile_v1_three_profs = true;
    uniwill_device_features.uniwill_has_double_pl4 = false;
    ec[0x0751] = 0xa5;
    ec[0x0727] = 0x40;
    event_count = 0;
}
static int find_event(int owner, char kind, u16 address, int after)
{
    for (int i = after + 1; i < event_count; ++i)
        if (events[i].actor == owner && events[i].kind == kind && events[i].address == address)
            return i;
    return -1;
}
static void assert_commit_active(int position, bool custom)
{
    assert(position >= 0);
    const struct event *event = &events[position];
    assert(event->status == 0 && event->ready && !event->suspended && !event->removed);
    if (custom)
        assert((event->mode & 0xb0) == 0x10);
}
static void interleave(enum operation second, bool custom)
{
    prepare(custom);
    pthread_t one = start(1, TDP);
    wait_pause();
    pthread_t two = start(2, second);
    bool contender_blocked = release_after_contender();
    assert(pthread_join(one, NULL) == 0 && pthread_join(two, NULL) == 0);
    assert(result[1] == 0 && result[2] == 0);
    /* No passing end-state can hide the contender running before the first
     * complete command. */
    assert(contender_blocked);
    assert(premature_command_releases[1] == 0 && premature_command_releases[2] == 0);
    int first_commit = find_event(1, 'w', 0x0783, -1);
    assert_commit_active(first_commit, custom);
    assert(events[first_commit].value == 85);
    if (custom) {
        int read = find_event(1, 'r', 0x0751, -1);
        int mode = find_event(1, 'w', 0x0751, -1);
        assert(read >= 0 && mode > read && first_commit > mode);
        assert(events[mode].value == 0x15);
    }
    for (int i = 0; i <= first_commit; ++i)
        assert(events[i].actor != 2);
    if (second == TDP) {
        int second_commit = find_event(2, 'w', 0x0783, first_commit);
        assert_commit_active(second_commit, custom);
        assert(events[second_commit].value == 102 && ec[0x0783] == 102);
        if (custom) {
            int read = find_event(2, 'r', 0x0751, first_commit);
            int mode = find_event(2, 'w', 0x0751, read);
            assert(read > first_commit && mode > read && second_commit > mode);
        }
    } else {
        assert(ec[0x0783] == 85);
        if (second == PROFILE)
            assert(ec[0x0751] == 0xa5);
        else if (second == RAW_MODE)
            assert(ec[0x0751] == 0x2f);
        else if (second == FULL_FAN)
            assert(ec[0x0751] == 0x55);
        else if (second == SUSPEND)
            assert(uniwill_pm_suspended && uniwill_pm_ready);
        else if (second == PREPARE_REMOVE)
            assert(!uniwill_pm_ready && uniwill_pm_suspended);
        else if (second == REMOVE)
            assert(uniwill_fan_removed && !uniwill_pm_ready && ec[0x0741] == 0);
    }
}
static void assert_reject_without_ec(int expected)
{
    int before = event_count;
    for (u8 index = 0; index < 3; ++index)
        assert(uw_set_tdp(index, 85) == expected);
    assert(event_count == before);
}
static void gate_case(const char *name, bool custom)
{
    prepare(custom);
    if (strcmp(name, "suspended") == 0) {
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        assert_reject_without_ec(-EBUSY);
    } else if (strcmp(name, "prepared") == 0) {
        uniwill_keyboard_prepare_remove();
        assert_reject_without_ec(-ENODEV);
    } else if (strcmp(name, "removed") == 0) {
        uniwill_keyboard_remove(NULL);
        assert_reject_without_ec(-ENODEV);
    } else if (strcmp(name, "unready") == 0) {
        assert(uw_fan_control_start() == 0);
        assert(!uniwill_pm_ready && !uniwill_pm_suspended && !uniwill_fan_removed);
        assert_reject_without_ec(-ENODEV);
    } else assert(0);
}
static void check_failure(const char *name, int error)
{
    bool custom = strcmp(name, "tdp_failure_noncustom") != 0;
    prepare(custom);
    failure_status = error;
    failure_occurrence = 1;
    failure_seen = 0;
    if (strcmp(name, "read_failure") == 0) {
        failure_kind = 'r'; failure_address = 0x0751;
    } else if (strcmp(name, "mode_failure") == 0) {
        failure_kind = 'w'; failure_address = 0x0751;
    } else {
        failure_kind = 'w'; failure_address = 0x0783;
    }
    ec[0x0783] = 77;
    assert(uw_set_tdp(0, 85) == error);
    assert(failure_seen == 1 && ec[0x0783] == 77);
    if (failure_kind == 'r') {
        assert(event_count == 1 && events[0].kind == 'r' && events[0].status == error);
        assert(ec[0x0751] == 0xa5);
    } else if (failure_address == 0x0751) {
        assert(event_count == 2 && events[0].kind == 'r' && events[0].status == 0);
        assert(events[1].kind == 'w' && events[1].address == 0x0751 && events[1].status == error);
        assert(ec[0x0751] == 0xa5);
    } else if (custom) {
        assert(event_count == 3);
        assert(events[0].kind == 'r' && events[0].address == 0x0751 && events[0].status == 0);
        assert(events[1].kind == 'w' && events[1].address == 0x0751 && events[1].status == 0);
        assert(events[2].kind == 'w' && events[2].address == 0x0783 && events[2].status == error);
        /* The accepted mode change has no known hardware rollback. */
        assert(ec[0x0751] == 0x15);
    } else {
        assert(event_count == 1 && events[0].kind == 'w' && events[0].address == 0x0783);
        assert(events[0].status == error && ec[0x0751] == 0xa5);
    }
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *test = argv[1];
    if (strcmp(test, "tdp_profile") == 0) interleave(PROFILE, true);
    else if (strcmp(test, "tdp_raw_mode") == 0) interleave(RAW_MODE, true);
    else if (strcmp(test, "tdp_full_fan") == 0) interleave(FULL_FAN, true);
    else if (strcmp(test, "tdp_suspend_custom") == 0) interleave(SUSPEND, true);
    else if (strcmp(test, "tdp_suspend_noncustom") == 0) interleave(SUSPEND, false);
    else if (strcmp(test, "tdp_prepare_custom") == 0) interleave(PREPARE_REMOVE, true);
    else if (strcmp(test, "tdp_prepare_noncustom") == 0) interleave(PREPARE_REMOVE, false);
    else if (strcmp(test, "tdp_remove_custom") == 0) interleave(REMOVE, true);
    else if (strcmp(test, "tdp_remove_noncustom") == 0) interleave(REMOVE, false);
    else if (strcmp(test, "tdp_tdp_custom") == 0) interleave(TDP, true);
    else if (strcmp(test, "tdp_tdp_noncustom") == 0) interleave(TDP, false);
    else if (strcmp(test, "suspended_custom") == 0) gate_case("suspended", true);
    else if (strcmp(test, "suspended_noncustom") == 0) gate_case("suspended", false);
    else if (strcmp(test, "prepared") == 0 || strcmp(test, "removed") == 0 ||
             strcmp(test, "unready") == 0) {
        gate_case(test, false);
        gate_case(test, true);
    } else if (strcmp(test, "resume_recovery") == 0) {
        for (int custom = 0; custom < 2; ++custom) {
            prepare(custom);
            assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            assert_reject_without_ec(-EBUSY);
            assert(uniwill_keyboard_resume(NULL) == 0);
            event_count = 0;
            assert(uw_set_tdp(0, 85) == 0);
            assert_commit_active(find_event(0, 'w', 0x0783, -1), custom);
        }
    } else if (strcmp(test, "read_failure") == 0 || strcmp(test, "mode_failure") == 0 ||
               strcmp(test, "tdp_failure_custom") == 0 || strcmp(test, "tdp_failure_noncustom") == 0) {
        const int errors[] = {-EIO, -ETIMEDOUT, -EOPNOTSUPP, -EPERM, -ENODEV};
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i)
            check_failure(test, errors[i]);
    } else if (strcmp(test, "failure_recovery") == 0) {
        const char *stages[] = {"read_failure", "mode_failure", "tdp_failure_custom", "tdp_failure_noncustom"};
        for (unsigned int i = 0; i < sizeof(stages) / sizeof(stages[0]); ++i) {
            check_failure(stages[i], -ETIMEDOUT);
            failure_kind = 0;
            assert(uw_set_tdp(0, 85) == 0 && ec[0x0783] == 85);
            assert(uw_set_mode(0x2f) == 0 && ec[0x0751] == 0x2f);
        }
    } else if (strcmp(test, "provider_invalid_index") == 0) {
#if HAS_TDP_PROVIDER
        prepare(true);
        const u8 invalid[] = {3, UINT8_MAX};
        for (unsigned int i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i)
            assert(uw_write_tdp(invalid[i], 85) == -EINVAL);
        assert(event_count == 0);
#else
        /* Python explicitly skips only this new exported-API case. */
        return 77;
#endif
    } else return 2;
    return 0;
}
