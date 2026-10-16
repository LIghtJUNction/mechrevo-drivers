/* The injected functions are compiled verbatim; no hardware or kernel is used. */
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
struct uniwill_device_features_t {
    bool uniwill_has_universal_ec_fan_control;
    bool uniwill_custom_profile_mode_needed;
};
static struct uniwill_device_features_t uniwill_device_features;
static struct uniwill_device_features_t *uniwill_get_device_features(void) { return &uniwill_device_features; }
static u8 direct_fan_control_current_value_fan0;
static u8 direct_fan_control_current_value_fan1;
static bool fans_initialized, direct_fan_control_started, direct_fan_control_restart_needed;
static bool direct_fan_control_suspend, universal_fan_manual_active;
static int uw_restore_fan_state_locked(void);
static u8 direct_fan_control_current_value_fan0_suspend_save;
static u8 direct_fan_control_current_value_fan1_suspend_save;
struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER }
static DEFINE_MUTEX(uniwill_fan_command_lock);
static DEFINE_MUTEX(uniwill_fan_state_lock);
static bool uniwill_fan_work_paused, uniwill_fan_removed;
static bool uniwill_pm_ready, uniwill_pm_suspended;
static void mutex_lock(struct mutex *lock) { assert(pthread_mutex_lock(&lock->native) == 0); }
static void mutex_unlock(struct mutex *lock) { assert(pthread_mutex_unlock(&lock->native) == 0); }
#define lockdep_assert_held(lock) do {} while (0)
struct platform_device { int unused; };
typedef int pm_message_t;
static int backlight_calls, backlight_enabled, leds_calls, priority_calls, profile_calls;
static int backlight_failures;
static int uniwill_write_kbd_bl_enable(u8 enabled) {
    ++backlight_calls;
    if (backlight_failures) { --backlight_failures; return -EIO; }
    backlight_enabled = enabled; return 0;
}
static void uniwill_leds_restore_state_extern(void) { ++leds_calls; }
static void uw_charging_priority_write_state(void) { ++priority_calls; }
static void uw_charging_profile_write_state(void) { ++profile_calls; }

struct work_struct { int unused; };
struct delayed_work { struct work_struct work; bool pending; };
static struct delayed_work direct_fan_control_restart_delayed_work;
struct event { char kind; u16 address; u8 value; };
static struct event events[1024];
static int event_count, reads, writes, sleeps, schedules, cancellations, errors;
static unsigned int sleep_values[128], schedule_values[8];
static u8 queued_fan0[8], queued_fan1[8];
static int fail_after, exact_fail_event, read_failures, write_failures, mismatch_reads, last_error, fault_status;
static u16 mismatch_address;
static int failed_commit_readbacks, recovery_write_failures, recovery_mismatches;
static bool commit_verification_exhausted, recovery_clear_written;
static u8 ec[UINT16_MAX + 1u];

static bool fault(bool write)
{
    if (fail_after >= 0 && event_count > fail_after)
        return true;
    if (event_count == exact_fail_event + 1)
        return true;
    int *remaining = write ? &write_failures : &read_failures;
    if (*remaining) {
        --*remaining;
        return true;
    }
    return false;
}
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
    assert(event_count < 1024);
    events[event_count++] = (struct event){'r', address, 0};
    ++reads;
    if (fault(false))
        return fault_status; /* Deliberately leave the output untouched on failure. */
    if (address == 0x07c6 && (ec[address] & 4) && ec[0x0f00] == 115 && failed_commit_readbacks) {
        if (!--failed_commit_readbacks) commit_verification_exhausted = true;
        return fault_status;
    }
    *value = ec[address];
    if (address == 0x07c6 && recovery_clear_written && recovery_mismatches) {
        --recovery_mismatches;
        *value ^= 4;
    }
    if (mismatch_reads && address == mismatch_address) {
        --mismatch_reads;
        *value ^= 1;
    }
    return 0;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
    assert(event_count < 1024);
    events[event_count++] = (struct event){'w', address, value};
    ++writes;
    if (fault(true))
        return fault_status;
    if (address == 0x07c6 && !(value & 4) && commit_verification_exhausted) {
        if (recovery_write_failures) {
            --recovery_write_failures;
            return -ENOSPC;
        }
        recovery_clear_written = true;
    }
    ec[address] = value;
    return 0;
}
static void msleep(unsigned int milliseconds)
{
    assert(sleeps < 128);
    sleep_values[sleeps++] = milliseconds;
}
static bool schedule_delayed_work(struct delayed_work *work, unsigned int delay)
{
    assert(work == &direct_fan_control_restart_delayed_work && schedules < 8);
    schedule_values[schedules] = delay;
    queued_fan0[schedules] = direct_fan_control_current_value_fan0;
    queued_fan1[schedules++] = direct_fan_control_current_value_fan1;
    if (work->pending)
        return false;
    work->pending = true;
    return true;
}
static void cancel_delayed_work_sync(struct delayed_work *work)
{
    assert(work == &direct_fan_control_restart_delayed_work);
    ++cancellations;
    work->pending = false;
}
#define msecs_to_jiffies(value) (value)
#define pr_debug(...) do {} while (0)
#define pr_err(format, status) do { ++errors; last_error = (status); } while (0)

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop

_Static_assert(sizeof(int) == sizeof(u32), "errno cast must keep the 32-bit ABI");
_Static_assert(_Generic(&uw_set_fan, u32 (*)(u32, u8): 1, default: 0),
               "exported fan ABI must remain u32(u32, u8)");
_Static_assert(_Generic(&uw_set_fan_auto, u32 (*)(void): 1, default: 0),
               "exported automatic fan ABI must remain u32(void)");

static void reset(bool universal)
{
    memset(ec, 0, sizeof(ec));
    memset(events, 0, sizeof(events));
    event_count = reads = writes = sleeps = schedules = cancellations = errors = 0;
    failed_commit_readbacks = recovery_write_failures = recovery_mismatches = 0;
    commit_verification_exhausted = recovery_clear_written = false;
    fail_after = -1;
    exact_fail_event = -1;
    fault_status = -ETIMEDOUT;
    read_failures = write_failures = mismatch_reads = last_error = 0;
    uniwill_device_features.uniwill_has_universal_ec_fan_control = universal;
    uniwill_device_features.uniwill_custom_profile_mode_needed = false;
    universal_fan_manual_active = false;
    fans_initialized = direct_fan_control_started = false;
    direct_fan_control_restart_needed = false;
    direct_fan_control_suspend = false;
    uniwill_fan_work_paused = uniwill_fan_removed = false;
    /* This isolated fan fixture starts in an already completed lifecycle. */
    uniwill_pm_ready = true;
    uniwill_pm_suspended = false;
    direct_fan_control_current_value_fan0_suspend_save = 0;
    direct_fan_control_current_value_fan1_suspend_save = 0;
    backlight_failures = 0;
    backlight_calls = leds_calls = priority_calls = profile_calls = 0;
    backlight_enabled = 1;
    direct_fan_control_restart_delayed_work.pending = false;
    direct_fan_control_current_value_fan0 = 83;
    direct_fan_control_current_value_fan1 = 97;
    ec[0x0751] = 0x51;
}
static void assert_cached(u8 fan0, u8 fan1)
{
    assert(direct_fan_control_current_value_fan0 == fan0);
    assert(direct_fan_control_current_value_fan1 == fan1);
}
static void run_worker(void)
{
    assert(direct_fan_control_restart_delayed_work.pending);
    direct_fan_control_restart_delayed_work.pending = false;
    restart_direct_fan_control_work_handler(&direct_fan_control_restart_delayed_work.work);
}
static void check_writes(const u16 *addresses, const u8 *values, int expected)
{
    int index = 0;
    for (int i = 0; i < event_count; ++i) {
        if (events[i].kind != 'w')
            continue;
        assert(index < expected && events[i].address == addresses[index]);
        assert(events[i].value == values[index++]);
    }
    assert(index == expected);
}
static void init_expected(u16 *addresses, u8 *values)
{
    int index = 0;
    addresses[index] = 0x0751; values[index++] = 0x11;
    addresses[index] = 0x07c5; values[index++] = 0x80;
    const u16 initial_addresses[] = {0x0f00, 0x0f10, 0x0f20, 0x0f30, 0x0f40, 0x0f50};
    const u8 initial_values[] = {115, 0, 1, 120, 0, 1};
    for (int i = 0; i < 6; ++i) {
        addresses[index] = initial_addresses[i]; values[index++] = initial_values[i];
    }
    for (int i = 1; i <= 15; ++i) {
        const u8 zone[] = {115 + i + 1, 115 + i, 200, 115 + i + 1, 115 + i, 200};
        for (int j = 0; j < 6; ++j) {
            addresses[index] = initial_addresses[j] + i; values[index++] = zone[j];
        }
    }
    addresses[index] = 0x07c6; values[index++] = 4;
    assert(index == 99);
}
static void assert_saved(u8 fan0, u8 fan1)
{
    assert(direct_fan_control_current_value_fan0_suspend_save == fan0);
    assert(direct_fan_control_current_value_fan1_suspend_save == fan1);
}
static void clear_trace(void)
{
    event_count = reads = writes = sleeps = schedules = errors = last_error = 0;
    fail_after = exact_fail_event = -1;
}
static void prepare_universal(void)
{
    reset(true); ec[0x0751] = 0x11;
    assert(uw_set_fan(0, 83) == 0 && uw_set_fan(1, 97) == 0);
    assert(universal_fan_manual_active && fans_initialized);
    assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending && schedules == 0);
    clear_trace();
}
static void lose_universal_firmware_state(void)
{
    memset(ec + 0x0f00, 0, 0x60);
    ec[0x07c5] &= 0x7f; ec[0x07c6] &= 0xfb;
    ec[0x1804] = ec[0x1809] = 0;
}
static void assert_universal_targets(u8 cpu, u8 gpu)
{
    assert(ec[0x0f20] == cpu && ec[0x1804] == cpu);
    assert(ec[0x0f50] == gpu && ec[0x1809] == gpu);
    assert((ec[0x07c5] & 0x80) && (ec[0x07c6] & 4));
    assert(universal_fan_manual_active && fans_initialized && !direct_fan_control_suspend);
    assert(!direct_fan_control_started && !direct_fan_control_restart_needed);
    assert(schedules == 0 && !direct_fan_control_restart_delayed_work.pending);
}
static void prepare_manual(void)
{
    reset(false);
    assert(uw_set_fan(0, 83) == 0 && uw_set_fan(1, 97) == 0);
    run_worker();
    assert(ec[0x0751] == 0x51 && ec[0x1804] == 83 && ec[0x1809] == 97);
}
static void prepare_suspended(void)
{
    prepare_manual();
    assert(uniwill_keyboard_suspend(NULL, 0) == 0);
    assert(direct_fan_control_suspend && !direct_fan_control_started);
    assert_saved(83, 97); assert_cached(0, 0);
    assert(!direct_fan_control_restart_delayed_work.pending && backlight_enabled == 0);
}

int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *test = argv[1];
    reset(true);
    if (strcmp(test, "retry_invalid") == 0) {
        assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 0) == -EINVAL);
        assert(uniwill_write_ec_ram_with_retry(0x1234, 42, -3) == -EINVAL);
        assert(event_count == 0 && sleeps == 0);
    } else if (strcmp(test, "retry_errors") == 0) {
        write_failures = 3;
        assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 3) == -ETIMEDOUT);
        assert(writes == 3 && reads == 0 && sleeps == 3);
        reset(true);
        read_failures = 3;
        assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 3) == -ETIMEDOUT);
        assert(writes == 3 && reads == 3 && sleeps == 3);
        for (int i = 0; i < sleeps; ++i)
            assert(sleep_values[i] == 50);
    } else if (strcmp(test, "retry_mismatch") == 0) {
        mismatch_address = 0x1234; mismatch_reads = 3;
        assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 3) == -EIO);
        assert(writes == 3 && reads == 3 && sleeps == 3);
    } else if (strcmp(test, "retry_recovers") == 0) {
        for (int kind = 0; kind < 3; ++kind) {
            reset(true);
            if (kind == 0) write_failures = 2;
            if (kind == 1) read_failures = 2;
            if (kind == 2) { mismatch_address = 0x1234; mismatch_reads = 2; }
            assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 3) == 0);
            assert(writes == 3 && reads == (kind == 0 ? 1 : 3) && sleeps == 2);
            assert(ec[0x1234] == 42);
        }
    } else if (strcmp(test, "errno_propagation") == 0) {
        const int errnos[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (int i = 0; i < 3; ++i) {
            reset(true); fault_status = errnos[i]; write_failures = 3;
            assert(uniwill_write_ec_ram_with_retry(0x1234, 42, 3) == errnos[i]);
            reset(true); fault_status = errnos[i]; read_failures = 1;
            assert(set_full_fan_mode(false) == errnos[i]);
            reset(true); fault_status = errnos[i]; fail_after = 0;
            assert(uw_init_fan() == errnos[i]);
            reset(true); fault_status = errnos[i]; fail_after = 0;
            assert((int)uw_set_fan(0, 100) == errnos[i]);
            reset(true); fault_status = errnos[i]; fail_after = 0;
            assert((int)uw_set_fan_auto() == errnos[i]);
            reset(false); fault_status = errnos[i]; direct_fan_control_started = true;
            write_failures = 1;
            assert((int)uw_set_fan(0, 100) == errnos[i]);
            reset(false); fault_status = errnos[i]; fail_after = 0;
            assert((int)uw_set_fan_auto() == errnos[i]);
            reset(false); fault_status = errnos[i]; fail_after = 0;
            assert(uw_set_fan(0, 100) == 0); run_worker();
            assert(errors == 1 && last_error == errnos[i]);
        }
    } else if (strcmp(test, "mode_success") == 0) {
        for (int enabled = 0; enabled <= 1; ++enabled) {
            for (int current = 0; current <= 1; ++current) {
                reset(true); ec[0x0751] = 0x95 | (current ? 0x40 : 0);
                assert(set_full_fan_mode(enabled) == 0);
                assert(ec[0x0751] == (0x95 | (enabled ? 0x40 : 0)));
                assert(reads == 1 && writes == (enabled != current));
                assert(set_full_fan_mode(enabled) == 0 && reads == 2);
                assert(writes == (enabled != current));
            }
        }
    } else if (strcmp(test, "mode_errors") == 0) {
        read_failures = 1;
        assert(set_full_fan_mode(false) == -ETIMEDOUT && writes == 0);
        reset(true); write_failures = 1;
        assert(set_full_fan_mode(false) == -ETIMEDOUT && reads == 1 && writes == 1);
        assert(ec[0x0751] == 0x51);
    } else if (strcmp(test, "init_success") == 0) {
        u16 addresses[99]; u8 values[99]; init_expected(addresses, values);
        assert(uw_init_fan() == 0 && fans_initialized);
        check_writes(addresses, values, 99);
        assert(reads == 102 && writes == 99 && sleeps == 0);
        int count = event_count;
        assert(uw_init_fan() == 0 && event_count == count);
        assert_cached(83, 97);
    } else if (strcmp(test, "init_failures") == 0) {
        assert(uw_init_fan() == 0);
        int count = event_count;
        struct event reference[1024]; memcpy(reference, events, sizeof(reference));
        for (int failure = 0; failure < count; ++failure) {
            reset(true); fail_after = failure;
            assert(uw_init_fan() == -ETIMEDOUT && !fans_initialized);
            assert(event_count > failure && event_count <= failure + 5);
            for (int i = failure; i < event_count; ++i)
                assert(events[i].address == reference[failure].address);
            assert_cached(83, 97);
        }
    } else if (strcmp(test, "init_live_transaction") == 0) {
        ec[0x0751] = 0x11; ec[0x07c5] = 0x23; ec[0x07c6] = 0xa7;
        assert(uw_init_fan() == 0 && fans_initialized);
        assert(events[0].kind == 'r' && events[0].address == 0x07c6);
        assert(events[1].kind == 'w' && events[1].address == 0x07c6 && events[1].value == 0xa3);
        bool disabled = false; int table_writes = 0;
        for (int i = 0; i < event_count; ++i) {
            if (events[i].kind != 'w') continue;
            if (events[i].address == 0x07c6) {
                if (!(events[i].value & 4)) disabled = true;
                else { assert(disabled && table_writes == 96); disabled = false; }
            }
            if (events[i].address >= 0x0f00 && events[i].address <= 0x0f5f) {
                assert(disabled); ++table_writes;
            }
        }
        assert(table_writes == 96 && ec[0x07c6] == 0xa7 && ec[0x07c5] == 0xa3);
    } else if (strcmp(test, "init_live_failures") == 0) {
        ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
        assert(uw_init_fan() == 0);
        struct event reference[1024]; memcpy(reference, events, sizeof(reference));
        int count = event_count;
        for (int failure = 0; failure < count; ++failure) {
            reset(true); ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
            fail_after = failure;
            assert(uw_init_fan() == -ETIMEDOUT && !fans_initialized);
            for (int i = 0; i < event_count; ++i)
                if (events[i].kind == 'w' && events[i].address >= 0x0f00 && events[i].address <= 0x0f5f)
                    assert(failure >= 3); /* disabled write and its readback succeeded */
            if (failure >= 3 && !(reference[failure].address == 0x07c6 && reference[failure].kind == 'r' && failure == count-1))
                assert(!(ec[0x07c6] & 4));
            if (failure == count-1) assert(errors == 1); /* commit landed, recovery unavailable */
        }
        /* One transient commit write failure still permits verified success. */
        reset(true); ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
        exact_fail_event = count-2; /* first commit WRITE fails once, retry succeeds */
        assert(uw_init_fan() == 0 && fans_initialized);
        /* Commit writes all land but every verification read fails. Recovery
         * must clear the live table and retain the original transport errno. */
        const int statuses[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (unsigned int i = 0; i < sizeof(statuses)/sizeof(statuses[0]); ++i) {
            reset(true); ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
            fault_status = statuses[i]; failed_commit_readbacks = 3;
            assert(uw_init_fan() == statuses[i] && !fans_initialized);
            assert(ec[0x07c6] == 0xa3 && errors == 0 && failed_commit_readbacks == 0);
            assert(events[event_count-2].kind == 'w' && events[event_count-2].address == 0x07c6 && events[event_count-2].value == 0xa3);
            assert(events[event_count-1].kind == 'r' && events[event_count-1].address == 0x07c6);
            assert(uw_init_fan() == 0 && fans_initialized && ec[0x07c6] == 0xa7);
        }
    } else if (strcmp(test, "init_commit_recovery_write_failure") == 0) {
        ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
        fault_status = -ENODEV; failed_commit_readbacks = recovery_write_failures = 3;
        assert(uw_init_fan() == -ENODEV && !fans_initialized);
        assert(commit_verification_exhausted && !recovery_clear_written);
        assert(recovery_write_failures == 0 && ec[0x07c6] == 0xa7);
        assert(errors == 1 && last_error == -ENOSPC);
        for (int i = event_count-3; i < event_count; ++i)
            assert(events[i].kind == 'w' && events[i].address == 0x07c6 && events[i].value == 0xa3);
        /* Fault is exhausted: the ordinary explicit retry disables and commits. */
        assert(uw_init_fan() == 0 && fans_initialized && ec[0x07c6] == 0xa7);
        assert(errors == 1);
    } else if (strcmp(test, "init_commit_recovery_mismatch") == 0) {
        ec[0x0751] = 0x11; ec[0x07c6] = 0xa7;
        fault_status = -ETIMEDOUT; failed_commit_readbacks = recovery_mismatches = 3;
        assert(uw_init_fan() == -ETIMEDOUT && !fans_initialized);
        assert(commit_verification_exhausted && recovery_clear_written && recovery_mismatches == 0);
        /* Mock storage is disabled, but each readback disagreed. Production
         * must report unverified recovery rather than claiming success. */
        assert(ec[0x07c6] == 0xa3 && errors == 1 && last_error == -EIO);
        for (int i = event_count-6; i < event_count; i += 2) {
            assert(events[i].kind == 'w' && events[i].address == 0x07c6 && events[i].value == 0xa3);
            assert(events[i+1].kind == 'r' && events[i+1].address == 0x07c6);
        }
        assert(uw_init_fan() == 0 && fans_initialized && ec[0x07c6] == 0xa7);
        assert(errors == 1);
    } else if (strcmp(test, "init_boundaries") == 0) {
        ec[0x0751] = 0x11; ec[0x07c5] = 0xa3; ec[0x07c6] = 0x96;
        assert(uw_init_fan() == 0 && fans_initialized);
        assert(writes == 98 && ec[0x07c5] == 0xa3 && ec[0x07c6] == 0x96);
        reset(false);
        assert(uw_init_fan() == 0 && fans_initialized && event_count == 0);
    } else if (strcmp(test, "universal_speeds") == 0) {
        for (u32 fan = 0; fan < 2; ++fan) {
            for (int speed = 0; speed <= 200; ++speed) {
                reset(true); fans_initialized = true; ec[0x0751] = 0x11;
                u8 mapped = speed < 25 ? 1 : (speed < 50 ? 50 : speed);
                assert(uw_set_fan(fan, speed) == 0 && reads == 1 && writes == 2);
                const u16 addresses[] = {fan ? 0x0f50 : 0x0f20, fan ? 0x1809 : 0x1804};
                const u8 values[] = {mapped, mapped}; check_writes(addresses, values, 2);
                assert_cached(fan ? 83 : mapped, fan ? mapped : 97);
                assert(schedules == 0 && !direct_fan_control_started);
            }
        }
    } else if (strcmp(test, "fan_invalid") == 0) {
        for (int universal = 0; universal <= 1; ++universal) {
            reset(universal);
            assert((int)uw_set_fan(2, 50) == -EINVAL);
            assert((int)uw_set_fan(UINT32_MAX, 50) == -EINVAL);
            assert(event_count == 0 && schedules == 0 && !fans_initialized);
        }
        reset(true);
        for (int speed = 201; speed <= 255; ++speed)
            assert((int)uw_set_fan(0, speed) == -EINVAL);
        assert(event_count == 0 && schedules == 0 && !fans_initialized);
    } else if (strcmp(test, "fan_full_mode_busy") == 0) {
        const u8 speeds[] = {0, 24, 25, 49, 50, 100, 200};
        u8 previous_ec[sizeof(ec)];
        for (unsigned int mode = 0; mode <= UINT8_MAX; ++mode) {
            if (!(mode & 0x40))
                continue;
            for (int initialized = 0; initialized <= 1; ++initialized) {
                for (u32 fan = 0; fan < 2; ++fan) {
                    for (unsigned int speed = 0; speed < sizeof(speeds); ++speed) {
                        reset(true);
                        fans_initialized = initialized;
                        ec[0x0751] = mode;
                        ec[0x0f20] = 61; ec[0x0f50] = 73;
                        ec[0x1804] = 85; ec[0x1809] = 99;
                        memcpy(previous_ec, ec, sizeof(ec));
                        assert((int)uw_set_fan(fan, speeds[speed]) == -EBUSY);
                        assert(reads == 1 && writes == 0 && event_count == 1);
                        assert(events[0].kind == 'r' && events[0].address == 0x0751);
                        assert(memcmp(previous_ec, ec, sizeof(ec)) == 0);
                        assert_cached(83, 97);
                        assert(fans_initialized == (bool)initialized);
                        assert(schedules == 0 && cancellations == 0 && sleeps == 0);
                        assert(!direct_fan_control_started && !direct_fan_control_restart_needed);
                        assert(!direct_fan_control_restart_delayed_work.pending);
                    }
                }
            }
        }
        /* A later request can succeed once the conflicting mode is absent. */
        ec[0x0751] &= ~0x40;
        fans_initialized = true;
        assert(uw_set_fan(1, 100) == 0);
        assert(ec[0x0f50] == 100 && ec[0x1809] == 100);
        assert_cached(83, 100);
    } else if (strcmp(test, "fan_errors") == 0) {
        /* Include every initialization operation through the exported helper. */
        ec[0x0751] = 0x11;
        assert(uw_set_fan(0, 100) == 0);
        int count = event_count;
        for (int failure = 0; failure < count; ++failure) {
            reset(true); ec[0x0751] = 0x11; fail_after = failure;
            assert((int)uw_set_fan(0, 100) == -ETIMEDOUT);
            assert_cached(83, 97);
            assert(schedules == 0);
        }
        for (u32 fan = 0; fan < 2; ++fan) {
            for (int failure = 0; failure < 3; ++failure) {
                reset(true); fans_initialized = true; ec[0x0751] = 0x11; fail_after = failure;
                assert((int)uw_set_fan(fan, 100) == -ETIMEDOUT);
                assert(event_count == failure + 1); assert_cached(83, 97);
            }
        }
    } else if (strcmp(test, "legacy_speeds") == 0) {
        for (u32 fan = 0; fan < 2; ++fan) {
            for (int speed = 0; speed <= 255; ++speed) {
                reset(false); direct_fan_control_started = true;
                assert(uw_set_fan(fan, speed) == 0 && reads == 0 && writes == 1);
                assert(events[0].address == (fan ? 0x1809 : 0x1804));
                assert(events[0].value == speed);
                assert_cached(fan ? 83 : speed, fan ? speed : 97);
                assert(schedules == 0);
            }
        }
    } else if (strcmp(test, "direct_errors") == 0) {
        for (u32 fan = 0; fan < 2; ++fan) {
            for (int legacy = 0; legacy <= 1; ++legacy) {
                reset(false); direct_fan_control_started = true; write_failures = 1;
                assert(direct_fan_control(fan, 100, legacy) == -ETIMEDOUT);
                assert_cached(83, 97); assert(writes == 1 && schedules == 0);
                write_failures = 1;
                assert((int)uw_set_fan(fan, 100) == -ETIMEDOUT);
                assert_cached(83, 97);
            }
        }
        reset(false);
        assert(direct_fan_control(2, 100, false) == -EINVAL && event_count == 0);
    } else if (strcmp(test, "worker_queue") == 0) {
        reset(false);
        assert(uw_set_fan(0, 100) == 0 && event_count == 0 && schedules == 1);
        assert(schedule_values[0] == 0 && direct_fan_control_started);
        assert(!direct_fan_control_restart_needed);
        assert(direct_fan_control_restart_delayed_work.pending); assert_cached(100, 97);
        assert(uw_set_fan(1, 120) == 0 && writes == 1 && schedules == 1);
        assert_cached(100, 120);
    } else if (strcmp(test, "worker_success") == 0) {
        reset(false); assert(uw_set_fan(0, 100) == 0); run_worker();
        assert(reads == 2 && writes == 22 && sleeps == 11 && errors == 0);
        const u16 prefix_addresses[] = {0x0751, 0x0751};
        assert(events[1].address == prefix_addresses[0] && events[1].value == 0x11);
        assert(events[3].address == prefix_addresses[1] && events[3].value == 0x51);
        for (int i = 0; i < 20; ++i) {
            assert(events[4 + i].kind == 'w');
            assert(events[4 + i].address == (i % 2 ? 0x1809 : 0x1804));
            assert(events[4 + i].value == (i % 2 ? 97 : 100));
        }
        for (int i = 0; i < sleeps; ++i) assert(sleep_values[i] == 10);
        assert(schedules == 2 && schedule_values[1] == 50 * 60 * 1000);
        assert(direct_fan_control_started && direct_fan_control_restart_delayed_work.pending);
    } else if (strcmp(test, "worker_failures") == 0) {
        for (int failure = 0; failure < 24; ++failure) {
            reset(false); assert(uw_set_fan(0, 100) == 0); fail_after = failure; run_worker();
            assert(event_count == failure + 1 && errors == 1 && last_error == -ETIMEDOUT);
            assert(schedules == 2 && direct_fan_control_started);
            assert(direct_fan_control_restart_needed);
            assert(schedule_values[1] == 50 * 60 * 1000);
            assert(direct_fan_control_restart_delayed_work.pending);
            /* It may still be in manual mode; retain the suspend/auto recovery signal. */
            fail_after = -1;
            int count = event_count;
            assert(uw_set_fan(0, 110) == 0 && schedules == 3 && direct_fan_control_started);
            assert(event_count == count && schedule_values[2] == 0 && !direct_fan_control_restart_needed);
            run_worker();
            assert(ec[0x0751] == 0x51 && ec[0x1804] == 110 && ec[0x1809] == 97);
            assert(uw_set_fan_auto() == 0 && !direct_fan_control_started);
        }
    } else if (strcmp(test, "worker_periodic_recovery") == 0) {
        reset(false); assert(uw_set_fan(0, 100) == 0);
        fail_after = 2; run_worker();
        assert(direct_fan_control_restart_needed && ec[0x0751] == 0x11);
        fail_after = -1; run_worker();
        assert(!direct_fan_control_restart_needed && ec[0x0751] == 0x51);
        int queued = schedules;
        assert(uw_set_fan(0, 110) == 0 && schedules == queued && ec[0x1804] == 110);
    } else if (strcmp(test, "auto_success") == 0) {
        fans_initialized = true; ec[0x07c5] = 0xb3; ec[0x07c6] = 0xa7;
        assert(uw_set_fan_auto() == 0 && !fans_initialized);
        const u16 addresses[] = {0x07c6, 0x07c5};
        const u8 values[] = {0xa3, 0x33}; check_writes(addresses, values, 2);
        assert_cached(0, 0);
        reset(true);
        assert(uw_set_fan_auto() == 0 && reads == 2 && writes == 0); assert_cached(0, 0);
        reset(false); direct_fan_control_started = true;
        direct_fan_control_restart_needed = true;
        direct_fan_control_restart_delayed_work.pending = true;
        assert(uw_set_fan_auto() == 0 && ec[0x0751] == 0x11 && cancellations == 1);
        assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
        assert(!direct_fan_control_restart_needed);
        assert_cached(0, 0);
    } else if (strcmp(test, "auto_failures") == 0) {
        for (int failure = 0; failure < 6; ++failure) {
            reset(true); fans_initialized = true; ec[0x07c5] = 0xb3; ec[0x07c6] = 0xa7;
            fail_after = failure;
            assert((int)uw_set_fan_auto() == -ETIMEDOUT && !fans_initialized);
            assert_cached(83, 97);
            fail_after = -1; ec[0x0751] = 0x11;
            assert(uw_set_fan(0, 100) == 0 && fans_initialized);
            assert_cached(100, 97);
        }
    } else if (strcmp(test, "legacy_auto_errors") == 0) {
        for (int failure = 0; failure < 2; ++failure) {
            reset(false); direct_fan_control_started = true;
            direct_fan_control_restart_delayed_work.pending = true; fail_after = failure;
            assert((int)uw_set_fan_auto() == -ETIMEDOUT);
            assert(event_count == failure + 1 && cancellations == 1);
            assert(direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
            assert(direct_fan_control_restart_needed && schedules == 0);
            assert_cached(83, 97);
            fail_after = -1;
            int count = event_count;
            assert(uw_set_fan(1, 110) == 0 && schedules == 1 && schedule_values[0] == 0);
            assert(event_count == count && direct_fan_control_restart_delayed_work.pending);
            assert(direct_fan_control_started && !direct_fan_control_restart_needed);
            assert_cached(83, 110);
            assert(uw_set_fan_auto() == 0 && !direct_fan_control_started);
            assert(!direct_fan_control_restart_needed && !direct_fan_control_restart_delayed_work.pending);
            assert_cached(0, 0);
        }
    } else if (strcmp(test, "legacy_auto_queue_recovery") == 0) {
        for (int failure = 0; failure < 2; ++failure) {
            reset(false); ec[0x0751] = 0x11;
            assert(uw_set_fan(0, 100) == 0 && schedules == 1 && event_count == 0);
            fail_after = failure;
            assert((int)uw_set_fan_auto() == -ETIMEDOUT);
            assert(direct_fan_control_started && direct_fan_control_restart_needed);
            assert(!direct_fan_control_restart_delayed_work.pending && ec[0x0751] == 0x11);
            assert_cached(100, 97);
            int count = event_count; fail_after = -1;
            assert(uw_set_fan(0, 110) == 0 && schedules == 2 && schedule_values[1] == 0);
            assert(event_count == count && direct_fan_control_restart_delayed_work.pending);
            assert(!direct_fan_control_restart_needed && ec[0x0751] == 0x11);
            run_worker();
            assert(ec[0x0751] == 0x51 && ec[0x1804] == 110 && ec[0x1809] == 97);
            assert(schedules == 3 && schedule_values[2] == 50 * 60 * 1000);
            assert(uw_set_fan_auto() == 0 && !direct_fan_control_started);
            assert(!direct_fan_control_restart_needed && !direct_fan_control_restart_delayed_work.pending);
            assert_cached(0, 0);
        }
    } else if (strcmp(test, "pm_success") == 0) {
        prepare_suspended();
        int queue_index = schedules;
        assert(uniwill_keyboard_resume(NULL) == 0 && !direct_fan_control_suspend);
        assert(queued_fan0[queue_index] == 83 && queued_fan1[queue_index] == 97);
        assert_cached(83, 97); assert_saved(83, 97);
        assert(backlight_enabled == 1 && leds_calls == 1 && priority_calls == 1 && profile_calls == 1);
        run_worker();
        assert(ec[0x0751] == 0x51 && ec[0x1804] == 83 && ec[0x1809] == 97);
    } else if (strcmp(test, "pm_resume_second_fan_error") == 0) {
        prepare_suspended();
        int queue_index = schedules, begin = event_count;
        write_failures = 1;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && direct_fan_control_suspend);
        assert_cached(83, 97); assert_saved(83, 97);
        assert(queued_fan0[queue_index] == 83 && queued_fan1[queue_index] == 97);
        assert(backlight_enabled == 1 && leds_calls == 1 && priority_calls == 1 && profile_calls == 1);
        assert(!uniwill_pm_suspended && !direct_fan_control_restart_delayed_work.pending);
        assert(direct_fan_control_restart_needed);
        /* Resume is allowed to fail without a second PM callback. A new manual
         * request must queue recovery with both saved targets intact. */
        assert(uw_set_fan(0, 83) == 0);
        run_worker();
        for (int i = begin; i < event_count; ++i) {
            if (events[i].kind == 'w' && events[i].address == 0x1809)
                assert(events[i].value == 97);
        }
        assert(ec[0x1804] == 83 && ec[0x1809] == 97 && direct_fan_control_suspend);
        assert(uniwill_keyboard_resume(NULL) == 0 && !direct_fan_control_suspend);
        assert_saved(83, 97);
    } else if (strcmp(test, "pm_suspend_fan_errors") == 0) {
        const int errnos[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (int error = 0; error < 3; ++error) {
            for (int failure = 0; failure < 2; ++failure) {
                prepare_manual(); fault_status = errnos[error]; fail_after = event_count + failure;
                assert(uniwill_keyboard_suspend(NULL, 0) == errnos[error]);
                assert(direct_fan_control_suspend && direct_fan_control_started);
                assert_cached(83, 97); assert_saved(83, 97);
                assert(direct_fan_control_restart_delayed_work.pending && backlight_enabled == 1);
                assert(backlight_calls == 0 && errors == 1);
                fail_after = -1; run_worker();
                assert(ec[0x0751] == 0x51 && ec[0x1804] == 83 && ec[0x1809] == 97);
                assert(uniwill_keyboard_resume(NULL) == 0 && !direct_fan_control_suspend);
            }
        }
    } else if (strcmp(test, "pm_resume_retry_errors") == 0) {
        const int errnos[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (int error = 0; error < 3; ++error) {
            prepare_suspended(); fault_status = errnos[error];
            for (int attempt = 1; attempt <= 3; ++attempt) {
                write_failures = 1;
                assert(uniwill_keyboard_resume(NULL) == errnos[error] && direct_fan_control_suspend);
                assert_cached(83, 97); assert_saved(83, 97);
                assert(backlight_enabled == 1 && leds_calls == attempt);
                assert(priority_calls == attempt && profile_calls == attempt);
                assert(!uniwill_pm_suspended && !direct_fan_control_restart_delayed_work.pending);
                assert(direct_fan_control_restart_needed);
            }
            assert(uniwill_keyboard_resume(NULL) == 0 && !direct_fan_control_suspend);
            run_worker();
            assert(ec[0x1804] == 83 && ec[0x1809] == 97);
        }
    } else if (strcmp(test, "pm_repeated_suspend") == 0) {
        prepare_suspended();
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        assert_saved(83, 97); assert_cached(0, 0);
        write_failures = 1;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && direct_fan_control_suspend);
        assert(uw_set_fan(0, 110) == 0); assert_saved(110, 97);
        assert(uniwill_keyboard_suspend(NULL, 0) == 0); assert_saved(110, 97);
        assert_cached(0, 0);
        assert(uniwill_keyboard_resume(NULL) == 0); run_worker();
        assert(ec[0x1804] == 110 && ec[0x1809] == 97 && !direct_fan_control_suspend);
    } else if (strcmp(test, "pm_profile_errors") == 0) {
        reset(false); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
        ec[0x0727] = 0xe5;
        assert(uniwill_keyboard_suspend(NULL, 0) == 0 && ec[0x0727] == 0xa5);
        assert(uniwill_keyboard_resume(NULL) == 0 && ec[0x0727] == 0xe5);
        for (int failure = 0; failure < 2; ++failure) {
            reset(false); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            ec[0x0727] = 0xe5; fail_after = failure;
            assert(uniwill_keyboard_suspend(NULL, 0) == -ETIMEDOUT);
            assert(event_count == failure + 1 && backlight_calls == 0 && backlight_enabled == 1);
            assert(ec[0x0727] == 0xe5);
            reset(false); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            ec[0x0727] = 0xa5; fail_after = failure; backlight_enabled = 0;
            assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
            assert(event_count == failure + 1 && backlight_enabled == 1);
            assert(leds_calls == 1 && priority_calls == 1 && profile_calls == 1);
            assert(ec[0x0727] == 0xa5);
        }
    } else if (strcmp(test, "pm_suspend_abort_recovery") == 0) {
        /* The failing device need not receive resume; its own abort queues recovery. */
        for (int failure = 0; failure < 4; ++failure) {
            prepare_manual(); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            ec[0x0727] = 0xe5; exact_fail_event = event_count + failure;
            assert(uniwill_keyboard_suspend(NULL, 0) == -ETIMEDOUT);
            assert(backlight_calls == 0 && backlight_enabled == 1);
            assert_cached(83, 97); assert_saved(83, 97);
            assert(direct_fan_control_restart_delayed_work.pending);
            assert(!direct_fan_control_suspend && errors == 0);
            run_worker();
            assert(ec[0x0751] == 0x51 && ec[0x1804] == 83 && ec[0x1809] == 97);
            assert(leds_calls == 0 && priority_calls == 0 && profile_calls == 0);
        }
    } else if (strcmp(test, "pm_explicit_commands") == 0) {
        prepare_suspended();
        int before = event_count, previous_schedules = schedules;
        assert(uniwill_pm_suspended);
        assert((int)uw_set_fan(0, 110) == -EBUSY);
        assert((int)uw_set_fan_auto() == -EBUSY);
        assert(uw_init_fan() == -EBUSY && set_full_fan_mode(true) == -EBUSY);
        assert(uw_set_mode(0x55) == -EBUSY && uw_update_mode(0xb0, 0xa0) == -EBUSY);
        assert(event_count == before && schedules == previous_schedules);
        assert_saved(83, 97); assert_cached(0, 0);
        assert(uniwill_keyboard_resume(NULL) == 0 && !uniwill_pm_suspended);
        assert(uw_set_fan(0, 110) == 0); assert_cached(110, 97);
        run_worker();
        assert(ec[0x1804] == 110 && ec[0x1809] == 97);
        assert(uw_set_fan_auto() == 0 && !direct_fan_control_suspend);
        assert_cached(0, 0);

        /* A failed resume also leaves ordinary commands usable, and the
         * explicit AUTO request cancels saved restoration rather than
         * requiring another resume to complete. */
        prepare_suspended(); write_failures = 1;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && !uniwill_pm_suspended);
        assert(direct_fan_control_suspend && !direct_fan_control_restart_delayed_work.pending);
        assert(uw_set_fan(0, 110) == 0); assert_cached(110, 97); assert_saved(110, 97);
        run_worker();
        assert(ec[0x1804] == 110 && ec[0x1809] == 97);
        assert(uw_set_fan_auto() == 0 && !direct_fan_control_suspend);
        assert_cached(0, 0);
        int count = event_count, queued = schedules;
        assert(uniwill_keyboard_suspend(NULL, 0) == 0 && uniwill_keyboard_resume(NULL) == 0);
        assert(event_count == count && schedules == queued && ec[0x0751] == 0x11);
        assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
    } else if (strcmp(test, "universal_pm_roundtrip") == 0) {
        prepare_universal();
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        assert(direct_fan_control_suspend && !universal_fan_manual_active && !fans_initialized);
        assert_saved(83, 97); assert_cached(0, 0);
        assert(!(ec[0x07c6] & 4) && !(ec[0x07c5] & 0x80));
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0);
        assert_universal_targets(83, 97);
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        assert(uniwill_keyboard_suspend(NULL, 0) == 0); /* repeat cannot overwrite saved */
        assert_saved(83, 97);
        assert(uniwill_keyboard_resume(NULL) == 0);
        assert_universal_targets(83, 97);
        clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0);
        assert_universal_targets(83, 97);
        assert(uw_set_fan(0, 110) == 0); assert_universal_targets(110, 97);
    } else if (strcmp(test, "universal_pm_init_partial") == 0) {
        reset(true); ec[0x0751] = 0x11;
        direct_fan_control_current_value_fan0 = direct_fan_control_current_value_fan1 = 0;
        assert(uw_init_fan() == 0 && universal_fan_manual_active);
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0);
        assert_universal_targets(1, 1);
        for (int failure = 1; failure <= 2; ++failure) {
            prepare_universal();
            fail_after = failure; /* target-table write or direct-target write */
            assert((int)uw_set_fan(0, 110) == -ETIMEDOUT);
            assert(universal_fan_manual_active); assert_cached(83, 97);
            clear_trace(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            assert_saved(83, 97); lose_universal_firmware_state(); clear_trace();
            assert(uniwill_keyboard_resume(NULL) == 0);
            assert_universal_targets(83, 97);
        }
    } else if (strcmp(test, "universal_pm_uncertain_commit") == 0) {
        reset(true); ec[0x0751] = 0x11;
        direct_fan_control_current_value_fan0 = direct_fan_control_current_value_fan1 = 0;
        failed_commit_readbacks = recovery_write_failures = 3;
        assert(uw_init_fan() == -ETIMEDOUT && !fans_initialized);
        assert(universal_fan_manual_active && (ec[0x07c6] & 4));
        clear_trace(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        assert(!(ec[0x07c6] & 4) && !universal_fan_manual_active);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0); assert_universal_targets(1, 1);
    } else if (strcmp(test, "universal_pm_restore_failures") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0);
        int count = event_count;
        for (int failure = 0; failure < count; ++failure) {
            prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            lose_universal_firmware_state(); clear_trace(); fail_after = failure;
            assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
            assert(direct_fan_control_suspend && !uniwill_pm_suspended);
            assert_saved(83, 97);
            assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending && schedules == 0);
            clear_trace(); assert(uw_set_fan(0, 110) == 0);
            assert_universal_targets(110, 97);
        }
    } else if (strcmp(test, "universal_pm_partial_auto_retry") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0);
        int final_gpu_write = -1;
        for (int i = 0; i < event_count; ++i)
            if (events[i].kind == 'w' && events[i].address == 0x1809) final_gpu_write = i;
        assert(final_gpu_write >= 0);
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace(); exact_fail_event = final_gpu_write;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
        assert(direct_fan_control_suspend && !universal_fan_manual_active && !fans_initialized);
        assert(!(ec[0x07c6] & 4) && errors == 0); assert_saved(83, 97);
        clear_trace();
        assert(uniwill_keyboard_suspend(NULL, 0) == 0); /* pending survives AUTO cache0 */
        assert_saved(83, 97);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0); assert_universal_targets(83, 97);
        clear_trace(); assert(uw_set_fan_auto() == 0);
        assert(!universal_fan_manual_active && !direct_fan_control_suspend && !fans_initialized);
        assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace();
        assert(uniwill_keyboard_resume(NULL) == 0 && !universal_fan_manual_active);
        assert(writes == 0 && schedules == 0);
    } else if (strcmp(test, "universal_pm_suspend_failures") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        int count = event_count;
        for (int failure = 0; failure < count; ++failure) {
            prepare_universal(); fail_after = failure;
            assert(uniwill_keyboard_suspend(NULL, 0) == -ETIMEDOUT && !uniwill_pm_suspended);
            if (failure) { assert(direct_fan_control_suspend); assert_saved(83, 97); }
            else { assert(universal_fan_manual_active); assert_cached(83, 97); }
            assert(schedules == 0 && !direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
            clear_trace(); assert(uw_set_fan(0, 110) == 0);
            assert_universal_targets(110, 97);
        }
        for (int stage = 0; stage < 3; ++stage) {
            prepare_universal();
            if (stage < 2) {
                uniwill_device_features.uniwill_custom_profile_mode_needed = true; ec[0x0727] = 0x40;
                assert(uniwill_keyboard_suspend(NULL, 0) == 0);
                int profile_event = -1;
                for (int i = 0; i < event_count; ++i)
                    if (events[i].address == 0x0727 && events[i].kind == (stage ? 'w' : 'r')) profile_event = i;
                assert(profile_event >= 0);
                prepare_universal(); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
                ec[0x0727] = 0x40; exact_fail_event = profile_event;
                assert(uniwill_keyboard_suspend(NULL, 0) == -ETIMEDOUT);
            } else {
                backlight_failures = 1;
                assert(uniwill_keyboard_suspend(NULL, 0) == -EIO);
            }
            assert(!uniwill_pm_suspended); assert_universal_targets(83, 97);
            assert_saved(83, 97); assert(errors == 0);
        }
    } else if (strcmp(test, "universal_pm_late_resume_failure") == 0) {
        for (int stage = 0; stage < 3; ++stage) {
            prepare_universal();
            uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            lose_universal_firmware_state(); clear_trace();
            assert(uniwill_keyboard_resume(NULL) == 0);
            int profile_event = -1;
            for (int i = 0; i < event_count; ++i)
                if (events[i].address == 0x0727 && events[i].kind == (stage ? 'w' : 'r')) profile_event = i;
            assert(profile_event >= 0);
            prepare_universal(); uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            lose_universal_firmware_state(); clear_trace();
            if (stage < 2) exact_fail_event = profile_event;
            else backlight_failures = 1;
            assert(uniwill_keyboard_resume(NULL) == (stage < 2 ? -ETIMEDOUT : -EIO));
            assert(direct_fan_control_suspend && !uniwill_pm_suspended); assert_saved(83, 97);
            clear_trace(); assert(uw_set_fan(0, 110) == 0);
            assert_universal_targets(110, 97);
        }
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace(); fail_after = 0;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && direct_fan_control_suspend);
        clear_trace(); fail_after = 0;
        assert((int)uw_set_fan_auto() == -ETIMEDOUT && direct_fan_control_suspend);
        assert_saved(83, 97);
        clear_trace(); assert(uw_set_fan_auto() == 0);
        assert(!direct_fan_control_suspend && !universal_fan_manual_active);
        clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0 && writes == 0);
    } else if (strcmp(test, "universal_pm_full_override") == 0) {
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        lose_universal_firmware_state(); clear_trace(); fail_after = 0;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && direct_fan_control_suspend);
        clear_trace(); assert(uw_set_mode(0x51) == 0);
        clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0);
        assert(!direct_fan_control_suspend && !universal_fan_manual_active && !(ec[0x07c6] & 4) && schedules == 0);
        for (int raw = 0; raw < 2; ++raw) {
            prepare_universal();
            if (raw) assert(uw_set_mode(0x51) == 0);
            else assert(set_full_fan_mode(true) == 0);
            clear_trace(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            assert(!direct_fan_control_suspend && !universal_fan_manual_active);
            assert(!(ec[0x07c6] & 4) && !(ec[0x07c5] & 0x80));
            assert(ec[0x0751] == 0x51);
            clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0);
            assert(writes == 0 && ec[0x0751] == 0x51 && schedules == 0);
            assert((int)uw_set_fan(0, 110) == -EBUSY);
            assert(uw_set_mode(0x11) == 0); /* raw FULL clear cannot revive old table */
            assert(!(ec[0x07c6] & 4));
            assert(uniwill_keyboard_suspend(NULL, 0) == 0 && !direct_fan_control_suspend);
            clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0 && writes == 0);
            clear_trace(); assert(uw_set_fan(0, 110) == 0 && universal_fan_manual_active);
            assert(ec[0x0f20] == 110 && ec[0x1804] == 110 && schedules == 0);
        }
    } else if (strcmp(test, "universal_pm_full_failures") == 0) {
        prepare_universal(); assert(set_full_fan_mode(true) == 0);
        clear_trace(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        int count = event_count;
        for (int failure = 0; failure < count; ++failure) {
            prepare_universal(); assert(set_full_fan_mode(true) == 0);
            clear_trace(); fail_after = failure;
            assert(uniwill_keyboard_suspend(NULL, 0) == -ETIMEDOUT);
            assert(universal_fan_manual_active && !uniwill_pm_suspended && ec[0x0751] == 0x51);
            clear_trace(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            assert(!universal_fan_manual_active && !(ec[0x07c6] & 4) && ec[0x0751] == 0x51);
        }
        /* FULL cancellation during resume must not be undone by a later
         * backlight failure using the callback's earlier pending snapshot. */
        prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
        clear_trace(); fail_after = 0;
        assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT && direct_fan_control_suspend);
        clear_trace(); assert(uw_set_mode(0x51) == 0);
        backlight_failures = 1;
        assert(uniwill_keyboard_resume(NULL) == -EIO);
        assert(!direct_fan_control_suspend && !universal_fan_manual_active && !(ec[0x07c6] & 4));
        assert(ec[0x0751] == 0x51 && schedules == 0);
        clear_trace(); assert(uw_set_mode(0x11) == 0);
        assert(uniwill_keyboard_resume(NULL) == 0 && !(ec[0x07c6] & 4));
        assert(!universal_fan_manual_active && !direct_fan_control_suspend);
        for (int stage = 0; stage < 2; ++stage) {
            prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            clear_trace(); fail_after = 0;
            assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
            clear_trace(); assert(uw_set_mode(0x51) == 0);
            uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            clear_trace(); assert(uniwill_keyboard_resume(NULL) == 0);
            int profile_event = -1;
            for (int i = 0; i < event_count; ++i)
                if (events[i].address == 0x0727 && events[i].kind == (stage ? 'w' : 'r')) profile_event = i;
            assert(profile_event >= 0);
            prepare_universal(); assert(uniwill_keyboard_suspend(NULL, 0) == 0);
            clear_trace(); fail_after = 0;
            assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
            clear_trace(); assert(uw_set_mode(0x51) == 0);
            uniwill_device_features.uniwill_custom_profile_mode_needed = true;
            clear_trace(); exact_fail_event = profile_event;
            assert(uniwill_keyboard_resume(NULL) == -ETIMEDOUT);
            assert(!direct_fan_control_suspend && !universal_fan_manual_active);
            assert(!(ec[0x07c6] & 4) && ec[0x0751] == 0x51 && schedules == 0);
        }
    } else if (strcmp(test, "mode_helpers_success") == 0) {
        const u8 profiles[] = {0xa0, 0x00, 0x10};
        for (int value = 0; value <= 255; ++value) {
            for (int profile = 0; profile < 3; ++profile) {
                reset(false); ec[0x0751] = value;
                assert(uw_update_mode(0xb0, profiles[profile]) == 0);
                assert(ec[0x0751] == ((value & ~0xb0) | profiles[profile]));
                assert(reads == 1 && writes == 1);
            }
            reset(false);
            assert(uw_set_mode(value) == 0 && ec[0x0751] == value);
            assert(reads == 0 && writes == 1);
        }
    } else if (strcmp(test, "mode_helpers_errors") == 0) {
        const int errnos[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (int error = 0; error < 3; ++error) {
            reset(false); fault_status = errnos[error]; read_failures = 1;
            assert(uw_update_mode(0xb0, 0xa0) == errnos[error]);
            assert(reads == 1 && writes == 0 && ec[0x0751] == 0x51);
            assert(uw_update_mode(0xb0, 0xa0) == 0 && ec[0x0751] == 0xe1);
            reset(false); fault_status = errnos[error]; write_failures = 1;
            assert(uw_update_mode(0xb0, 0xa0) == errnos[error]);
            assert(reads == 1 && writes == 1 && ec[0x0751] == 0x51);
            assert(uw_set_mode(0x55) == 0 && ec[0x0751] == 0x55);
            reset(false); fault_status = errnos[error]; write_failures = 1;
            assert(uw_set_mode(0x55) == errnos[error]);
            assert(reads == 0 && writes == 1 && ec[0x0751] == 0x51);
            assert(uw_set_mode(0x55) == 0 && ec[0x0751] == 0x55);
        }
    } else if (strcmp(test, "mode_helpers_removed") == 0) {
        uniwill_fan_removed = true;
        assert(uw_set_mode(0x55) == -ENODEV && uw_update_mode(0xb0, 0xa0) == -ENODEV);
        assert(event_count == 0 && cancellations == 0);
    } else if (strcmp(test, "start_errors") == 0) {
        const int errnos[] = {-EIO, -ENODEV, -ETIMEDOUT};
        for (int error = 0; error < 3; ++error) {
            reset(false); fault_status = errnos[error]; write_failures = 1;
            direct_fan_control_started = direct_fan_control_suspend = fans_initialized = true;
            direct_fan_control_restart_delayed_work.pending = true;
            assert(uw_fan_control_start() == errnos[error] && uniwill_fan_removed);
            assert(writes == 1 && reads == 0 && !direct_fan_control_restart_delayed_work.pending);
            assert(!direct_fan_control_started && !direct_fan_control_suspend && !fans_initialized);
            assert((int)uw_set_fan(0, 110) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
            assert(uw_init_fan() == -ENODEV && set_full_fan_mode(true) == -ENODEV);
            assert(writes == 1 && reads == 0);
            assert(uw_fan_control_start() == 0 && !uniwill_fan_removed);
            assert(!uniwill_pm_ready && !uniwill_pm_suspended && ec[0x0751] == 0);
            int before = event_count;
            assert((int)uw_set_fan(0, 110) == -ENODEV && (int)uw_set_fan_auto() == -ENODEV);
            assert(uw_init_fan() == -ENODEV && set_full_fan_mode(true) == -ENODEV);
            assert(uw_set_mode(0x55) == -ENODEV && uw_update_mode(0xb0, 0xa0) == -ENODEV);
            assert(event_count == before);
            assert(!direct_fan_control_started && !direct_fan_control_restart_delayed_work.pending);
        }
    } else if (strcmp(test, "invalid_preserves_recovery") == 0) {
        for (int universal = 0; universal < 2; ++universal) {
            reset(universal); direct_fan_control_started = direct_fan_control_restart_needed = true;
            direct_fan_control_restart_delayed_work.pending = true;
            assert((int)uw_set_fan(2, 100) == -EINVAL);
            if (universal) assert((int)uw_set_fan(0, 201) == -EINVAL);
            assert(event_count == 0 && cancellations == 0 && schedules == 0);
            assert(direct_fan_control_restart_delayed_work.pending && direct_fan_control_restart_needed);
            assert_cached(83, 97);
        }
    } else {
        return 2;
    }
    return 0;
}
