/* Complete real probe/filter/remove; only external kernel services are mocked. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define KERNEL_VERSION(a,b,c) (((a)<<16)|((b)<<8)|(c))
#define __always_unused __attribute__((unused))
#define unlikely(v) (v)
#define ARRAY_SIZE(a) (sizeof(a)/sizeof((a)[0]))
#define I8042_STR_AUXDATA 0x20
#define UW_EC_REG_KBD_BL_STATUS 0x078c
#define TUXEDO_ERROR(...) error_log(__VA_ARGS__)
#define pr_info(...) do {} while (0)
struct mutex { bool held; };
static struct mutex uniwill_fan_command_lock, uniwill_fan_state_lock;
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
#define lockdep_assert_held(m) assert((m)->held)
struct work_struct { bool pending; };
static struct work_struct uniwill_key_event_work;
struct device { struct { int unused; } kobj; };
struct platform_device { struct device dev; };
struct serio { int unused; };
struct uniwill_device_features_t { bool uniwill_profile_v1, uniwill_has_universal_ec_fan_control; };
static struct platform_device platform;
static struct uniwill_device_features_t features;
static bool uw_lightbar_loaded, uw_charging_prio_loaded, uw_charging_profile_loaded;
static bool uniwill_fan_removed, uniwill_fan_work_paused, fans_initialized;
static bool universal_fan_manual_active;
#define uniwill_device_features features
/* This probe-only fixture never starts RAM fan control. Complete production
 * AUTO and removal integration is exercised by uniwill_fan_concurrency.c. */
static u32 uw_set_fan_auto_locked(void) {
    assert(!"Unexpected owned RAM table in probe-only fixture"); return 0;
}
static bool direct_fan_control_started, direct_fan_control_restart_needed, direct_fan_control_suspend;
static u8 direct_fan_control_current_value_fan0, direct_fan_control_current_value_fan1;
static u8 direct_fan_control_current_value_fan0_suspend_save, direct_fan_control_current_value_fan1_suspend_save;
static int direct_fan_control_restart_delayed_work, quiesces;
static void cancel_delayed_work_sync(int *work) {
    assert(work == &direct_fan_control_restart_delayed_work);
    assert(uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
    ++quiesces;
}
static bool uniwill_pm_ready, uniwill_pm_suspended;
static __attribute__((unused)) int uw_charging_prio_attr_group, uw_charging_profile_attr_group;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
enum filter_slot { EMPTY, OURS, FOREIGN };
static enum filter_slot active_filter;
static int install_errno, start_errno, installations, removals, drains, schedules, tail_initializations;
struct operation { char kind; unsigned int address; u8 value; };
static struct operation trace[256];
static int operations, fail_at, fail_second, injected_errno, second_errno;
static int errors, error_codes[4], resources, resource_removals;
static u8 ec[0x800];
static void error_log(const char *format, ...) {
    assert(strstr(format, "%d") && errors < (int)ARRAY_SIZE(error_codes));
    va_list args; va_start(args, format); error_codes[errors++] = va_arg(args, int); va_end(args);
}
static int record(char kind, unsigned int address, u8 value) {
    assert(address < ARRAY_SIZE(ec) && operations < (int)ARRAY_SIZE(trace));
    trace[operations++] = (struct operation){kind, address, value};
    if (operations == fail_at) return injected_errno;
    if (operations == fail_second) return second_errno;
    return 0;
}
static bool deliver_on_install;
static bool uniwill_i8042_filter(unsigned char data, unsigned char str, struct serio *port
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 14, 0)
                                 , void *context
#endif
);
typedef bool (*filter_fn)(unsigned char, unsigned char, struct serio *
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 14, 0)
                         , void *
#endif
);
static void send_complete_sequence(void);
static int i8042_install_filter(filter_fn fn
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 14, 0)
                                , void *context
#endif
) {
    assert(fn == uniwill_i8042_filter);
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 14, 0)
    assert(context == NULL);
#endif
    ++installations;
    if (install_errno) return install_errno;
    assert(active_filter == EMPTY);
    active_filter = OURS;
    /* A real callback can run immediately after the core publishes it. */
    if (deliver_on_install) send_complete_sequence();
    return 0;
}
static int i8042_remove_filter(filter_fn fn) {
    assert(fn == uniwill_i8042_filter && active_filter == OURS);
    assert(uniwill_fan_command_lock.held);
    ++removals; active_filter = EMPTY; return 0;
}
static bool schedule_work(struct work_struct *work) {
    assert(work == &uniwill_key_event_work); ++schedules;
    bool fresh = !work->pending; work->pending = true; return fresh;
}
static void cancel_work_sync(struct work_struct *work) {
    assert(work == &uniwill_key_event_work && uniwill_fan_command_lock.held);
    ++drains; work->pending = false;
}
static void set_rom_id(void) {}
static struct uniwill_device_features_t *uniwill_get_device_features(void) { return &features; }
static int uniwill_read_ec_ram(unsigned int address, u8 *data) {
    int status = record('r', address, 0);
    if (!status) *data = ec[address];
    return status;
}
static int uniwill_write_ec_ram(unsigned int address, u8 data) {
    int status = record('w', address, data);
    if (address == 0x0751 && start_errno) status = start_errno;
    if (!status) ec[address] = data;
    return status;
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

static int uniwill_set_custom_profile_mode(bool enabled) { assert(enabled); ++tail_initializations; return 0; }
static void uniwill_leds_init(struct platform_device *dev) { assert(dev == &platform); ++resources; }
static int uw_lightbar_init(struct platform_device *dev) { assert(dev == &platform); return -ENODEV; }
#define INIT_STUB(name) static void name(struct platform_device *dev) { assert(dev == &platform); }
INIT_STUB(uw_charging_priority_init)
INIT_STUB(uw_charging_profile_init)
INIT_STUB(uw_ac_auto_boot_init)
INIT_STUB(uw_usb_powershare_init)
INIT_STUB(uw_mini_led_local_dimming_init)
static void uw_show_hidden_bios_options(void) {}
static void uw_battery_init(void) { ++resources; }
static void uw_battery_uninit(void) { ++resource_removals; }
static void sysfs_remove_group(void *kobj, void *group) { (void)kobj; (void)group; }
/* Charging lifecycle is exercised with its real implementation separately. */
static __attribute__((unused)) void uw_charging_uninit(struct platform_device *dev)
{
    (void)dev;
    if (uw_charging_prio_loaded) sysfs_remove_group(NULL, &uw_charging_prio_attr_group);
    if (uw_charging_profile_loaded) sysfs_remove_group(NULL, &uw_charging_profile_attr_group);
}
static void uniwill_leds_remove(struct platform_device *dev) { assert(dev == &platform); ++resource_removals; }
static void uw_lightbar_remove(struct platform_device *dev) { assert(dev == &platform); }
/* @DRIVER_FUNCTIONS@ */
static int invoke_backlight(u8 enable) { /* @INVOKE_BACKLIGHT@ */ }
static bool send_byte(u8 data, u8 str) {
    assert(active_filter == OURS);
#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 14, 0)
    return uniwill_i8042_filter(data, str, NULL);
#else
    return uniwill_i8042_filter(data, str, NULL, NULL);
#endif
}
static void send_complete_sequence(void) {
    for (unsigned int i = 0; i < ARRAY_SIZE(uniwill_touchp_toggle_seq); ++i)
        assert(send_byte(uniwill_touchp_toggle_seq[i], 0) == (i == 3 || i == 4));
}
static void remove_keyboard(void) {
#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 11, 0)
    assert(uniwill_keyboard_remove(&platform) == 0);
#else
    uniwill_keyboard_remove(&platform);
#endif
    assert(!uniwill_i8042_filter_installed && !uniwill_key_event_work.pending && uniwill_fan_removed);
    assert(!uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
}
static void check_install_error(int error, enum filter_slot slot) {
    install_errno = error; active_filter = slot;
    assert(uniwill_keyboard_probe(&platform) == 0);
    assert(!uniwill_i8042_filter_installed && active_filter == slot);
    int previous_removals = removals;
    /* Work cancellation is required even if this lifecycle owns no filter. */
    uniwill_key_event_work.pending = true;
    remove_keyboard();
    assert(removals == previous_removals && active_filter == slot);
    active_filter = EMPTY; install_errno = 0;
}
static const int ec_errors[] = {-EIO, -ENODEV, -ETIMEDOUT};
static void reset_test(void) {
    assert(!uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
    assert(active_filter == EMPTY && !uniwill_i8042_filter_installed);
    memset(ec, 0, sizeof(ec)); memset(trace, 0, sizeof(trace));
    for (int i = 0; i < 5; ++i) ec[0x0786 + i] = 0x91 + 7 * i;
    operations = fail_at = fail_second = errors = 0;
    installations = removals = drains = schedules = tail_initializations = 0;
    resources = resource_removals = quiesces = 0;
    install_errno = start_errno = 0;
    injected_errno = -EIO; second_errno = -ETIMEDOUT;
    features.uniwill_profile_v1 = false;
    uniwill_key_event_work.pending = false;
    uniwill_fan_removed = true;
    uniwill_kbd_bl_enable_state_on_start = 0xff;
}
static void check_operation(int index, char kind, unsigned int address, u8 value) {
    assert(index < operations);
    assert(trace[index].kind == kind && trace[index].address == address && trace[index].value == value);
}
static void assert_no_locks(void) {
    assert(!uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
}
static void helper_errors(bool write_error) {
    for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i) {
        reset_test(); ec[UW_EC_REG_KBD_BL_STATUS] = 0xd7;
        fail_at = write_error ? 2 : 1; injected_errno = ec_errors[i];
        assert(invoke_backlight(1) == injected_errno);
        assert(operations == fail_at && ec[UW_EC_REG_KBD_BL_STATUS] == 0xd7);
        check_operation(0, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
        if (write_error) check_operation(1, 'w', UW_EC_REG_KBD_BL_STATUS, 0xd5);
        assert_no_locks();
    }
}
static void critical_failure(int position, int error, bool retry, int cleanup_error) {
    reset_test(); features.uniwill_profile_v1 = true;
    fail_at = position; injected_errno = error;
    if (cleanup_error) { fail_second = position + 1; second_errno = cleanup_error; }
    assert(uniwill_keyboard_probe(&platform) == error);
    assert(operations == position + (position == 13));
    assert(uniwill_fan_removed && !resources && !resource_removals && !installations);
    assert(!uniwill_i8042_filter_installed && active_filter == EMPTY);
    assert(uniwill_kbd_bl_enable_state_on_start == 0xff && quiesces == (position == 1 ? 1 : 2));
    assert(tail_initializations == (position >= 12));
    assert(errors == (position != 1) + !!cleanup_error);
    if (position != 1) assert(error_codes[errors - 1] == error);
    if (cleanup_error) assert(error_codes[0] == cleanup_error);
    check_operation(0, 'w', 0x0751, 0);
    int count = 1;
    for (int i = 0; i < 5 && count < position; ++i) {
        check_operation(count++, 'r', 0x0786 + i, 0);
        if (count == position) break;
        check_operation(count++, 'w', 0x0743 + i, 0x91 + 7 * i);
    }
    if (position >= 12) check_operation(11, 'w', 0x0741, 1);
    if (position == 13) {
        check_operation(12, 'w', 0x044f, 0);
        check_operation(13, 'w', 0x0741, 0);
        assert(ec[0x0741] == !!cleanup_error);
    }
    for (int i = 0; i < 5; ++i)
        assert(ec[0x0743 + i] == (position > 3 + 2 * i ? 0x91 + 7 * i : 0));
    int previous = operations;
    assert(uw_fan_lock(false) == -ENODEV && operations == previous);
    assert_no_locks();
    if (retry) {
        fail_at = fail_second = 0; operations = errors = 0;
        assert(uniwill_keyboard_probe(&platform) == 0 && !uniwill_fan_removed);
        assert(resources == 2 && installations == 1);
        remove_keyboard(); assert(resource_removals == 2);
    }
}
static void optional_initial_read_errors(void) {
    for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i) {
        reset_test(); fail_at = 4; injected_errno = ec_errors[i];
        ec[UW_EC_REG_KBD_BL_STATUS] = 0x22;
        assert(uniwill_keyboard_probe(&platform) == 0);
        assert(uniwill_kbd_bl_enable_state_on_start == 0xff);
        assert(errors == 1 && error_codes[0] == injected_errno);
        assert(resources == 2 && installations == 1 && operations == 6);
        check_operation(3, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
        check_operation(4, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
        check_operation(5, 'w', UW_EC_REG_KBD_BL_STATUS, 0x20);
        remove_keyboard(); assert(operations == 7);
        check_operation(6, 'w', 0x0741, 0);
    }
}
static void optional_enable_errors(void) {
    for (int position = 5; position <= 6; ++position)
        for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i) {
            reset_test(); fail_at = position; injected_errno = ec_errors[i];
            ec[UW_EC_REG_KBD_BL_STATUS] = 0xa6;
            assert(uniwill_keyboard_probe(&platform) == 0);
            assert(uniwill_kbd_bl_enable_state_on_start == 0);
            assert(errors == 1 && error_codes[0] == injected_errno && operations == position);
            assert(ec[UW_EC_REG_KBD_BL_STATUS] == 0xa6);
            assert(resources == 2 && installations == 1);
            fail_at = 0; remove_keyboard();
            assert(resource_removals == 2);
        }
}
static void remove_error(int phase) {
    for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i) {
        reset_test(); assert(uniwill_keyboard_probe(&platform) == 0);
        assert(operations == 6);
        fail_at = operations + phase; injected_errno = ec_errors[i];
        uniwill_key_event_work.pending = true;
        remove_keyboard();
        assert(errors == 1 && error_codes[0] == injected_errno);
        assert(operations == 6 + (phase == 1 ? 2 : 3));
        assert(removals == 1 && drains == 1 && resource_removals == 2);
        check_operation(operations - 1, 'w', 0x0741, 0);
    }
}
static bool ec_case(const char *test) {
    if (!strcmp(test, "backlight_bytes")) {
        for (int value = 0; value <= 255; ++value)
            for (int enable = 0; enable <= 255; ++enable) {
                reset_test(); ec[UW_EC_REG_KBD_BL_STATUS] = value;
                assert(invoke_backlight(enable) == 0 && operations == 2);
                assert(ec[UW_EC_REG_KBD_BL_STATUS] == ((value & ~2) | (!(enable & 1) << 1)));
                check_operation(0, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
                check_operation(1, 'w', UW_EC_REG_KBD_BL_STATUS, (value & ~2) | (!(enable & 1) << 1));
            }
    } else if (!strcmp(test, "backlight_read_errors")) helper_errors(false);
    else if (!strcmp(test, "backlight_write_errors")) helper_errors(true);
    else if (!strcmp(test, "curve_success")) {
        reset_test(); features.uniwill_profile_v1 = true;
        assert(uniwill_keyboard_probe(&platform) == 0 && operations == 16);
        check_operation(0, 'w', 0x0751, 0);
        for (int i = 0; i < 5; ++i) {
            check_operation(1 + 2 * i, 'r', 0x0786 + i, 0);
            check_operation(2 + 2 * i, 'w', 0x0743 + i, 0x91 + 7 * i);
        }
        check_operation(11, 'w', 0x0741, 1); check_operation(12, 'w', 0x044f, 0);
        check_operation(13, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
        check_operation(14, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
        check_operation(15, 'w', UW_EC_REG_KBD_BL_STATUS, 0);
        assert(!errors && resources == 2 && tail_initializations == 1 && !uniwill_fan_removed);
        remove_keyboard();
    } else if (!strcmp(test, "curve_errors") || !strcmp(test, "manual_enable_errors") ||
               !strcmp(test, "fan_detection_errors") || !strcmp(test, "critical_reprobe")) {
        int begin = !strcmp(test, "curve_errors") ? 2 : !strcmp(test, "manual_enable_errors") ? 12 :
                    !strcmp(test, "fan_detection_errors") ? 13 : 1;
        int end = !strcmp(test, "curve_errors") ? 11 : !strcmp(test, "critical_reprobe") ? 13 : begin;
        for (int position = begin; position <= end; ++position)
            for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i)
                critical_failure(position, ec_errors[i], !strcmp(test, "critical_reprobe"), 0);
    } else if (!strcmp(test, "cleanup_errors")) {
        for (unsigned int i = 0; i < ARRAY_SIZE(ec_errors); ++i)
            for (unsigned int j = 0; j < ARRAY_SIZE(ec_errors); ++j)
                critical_failure(13, ec_errors[i], false, ec_errors[j]);
    } else if (!strcmp(test, "initial_backlight_errors")) optional_initial_read_errors();
    else if (!strcmp(test, "enable_backlight_errors")) optional_enable_errors();
    else if (!strcmp(test, "backlight_restore_bytes")) {
        for (int initial = 0; initial <= 255; ++initial)
            for (int current = 0; current <= 255; ++current) {
                reset_test(); ec[UW_EC_REG_KBD_BL_STATUS] = initial;
                assert(uniwill_keyboard_probe(&platform) == 0 && operations == 6);
                assert(uniwill_kbd_bl_enable_state_on_start == !(initial & 2));
                ec[UW_EC_REG_KBD_BL_STATUS] = current;
                remove_keyboard(); assert(operations == 9);
                assert(ec[UW_EC_REG_KBD_BL_STATUS] == ((current & ~2) | (initial & 2)));
                check_operation(6, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
                check_operation(7, 'w', UW_EC_REG_KBD_BL_STATUS, (current & ~2) | (initial & 2));
                check_operation(8, 'w', 0x0741, 0);
            }
    } else if (!strcmp(test, "restore_errors")) { remove_error(1); remove_error(2); }
    else if (!strcmp(test, "remove_manual_errors")) remove_error(3);
    else if (!strcmp(test, "backlight_reprobe")) {
        for (int enabled = 0; enabled <= 1; ++enabled) {
            reset_test(); ec[UW_EC_REG_KBD_BL_STATUS] = !enabled << 1;
            assert(uniwill_keyboard_probe(&platform) == 0);
            assert(uniwill_kbd_bl_enable_state_on_start == enabled);
            remove_keyboard();
            start_errno = -EIO;
            assert(uniwill_keyboard_probe(&platform) == -EIO);
            assert(uniwill_kbd_bl_enable_state_on_start == 0xff);
            start_errno = 0; operations = 0; fail_at = 4;
            assert(uniwill_keyboard_probe(&platform) == 0);
            assert(uniwill_kbd_bl_enable_state_on_start == 0xff);
            remove_keyboard(); assert(operations == 7);
        }
    } else return false;
    return true;
}
int main(int argc, char **argv) {
    assert(argc == 2); const char *test = argv[1];
    if (ec_case(test)) return 0;
    if (!strcmp(test, "install_errors")) {
        int errors[] = { -EBUSY, -ENOMEM, -ENODEV, -EIO };
        for (unsigned int i = 0; i < ARRAY_SIZE(errors); ++i) check_install_error(errors[i], EMPTY);
        assert(installations == 4 && removals == 0 && drains == 4); return 0;
    }
    if (!strcmp(test, "foreign")) {
        check_install_error(-EBUSY, FOREIGN);
        assert(installations == 1 && removals == 0 && drains == 1); return 0;
    }
    if (!strcmp(test, "early_errors")) {
        int errors[] = { -ETIMEDOUT, -ENODEV, -EIO };
        for (unsigned int i = 0; i < ARRAY_SIZE(errors); ++i) {
            start_errno = errors[i]; assert(uniwill_keyboard_probe(&platform) == errors[i]);
            assert(!uniwill_i8042_filter_installed && active_filter == EMPTY);
        }
        assert(installations == 0 && tail_initializations == 0 && removals == 0); return 0;
    }
    assert(uniwill_keyboard_probe(&platform) == 0);
    assert(uniwill_i8042_filter_installed && active_filter == OURS && installations == 1);
    if (!strcmp(test, "sequence")) {
        assert(!send_byte(0x76, I8042_STR_AUXDATA));
        assert(!send_byte(0x01, 0));
        for (unsigned int i = 0; i < ARRAY_SIZE(uniwill_touchp_toggle_seq); ++i) {
            if (i == 3) assert(!send_byte(0x01, I8042_STR_AUXDATA));
            assert(send_byte(uniwill_touchp_toggle_seq[i], 0) == (i == 3 || i == 4));
        }
        send_complete_sequence(); assert(schedules == 2);
        remove_keyboard(); return 0;
    }
    if (!strcmp(test, "prefixes")) {
        for (unsigned int length = 1; length < ARRAY_SIZE(uniwill_touchp_toggle_seq); ++length) {
            for (unsigned int i = 0; i < length; ++i) (void)send_byte(uniwill_touchp_toggle_seq[i], 0);
            int previous = schedules; remove_keyboard();
            deliver_on_install = true;
            assert(uniwill_keyboard_probe(&platform) == 0);
            assert(schedules == previous + 1 && uniwill_key_event_work.pending);
            deliver_on_install = false;
        }
        remove_keyboard(); assert(removals == 8 && installations == 8); return 0;
    }
    if (!strcmp(test, "stale_tail")) {
        for (unsigned int i = 0; i < ARRAY_SIZE(uniwill_touchp_toggle_seq) - 1; ++i)
            (void)send_byte(uniwill_touchp_toggle_seq[i], 0);
        remove_keyboard(); assert(uniwill_keyboard_probe(&platform) == 0);
        assert(!send_byte(0xdb, 0)); assert(schedules == 0 && !uniwill_key_event_work.pending);
        send_complete_sequence(); assert(schedules == 1); remove_keyboard(); return 0;
    }
    send_complete_sequence(); assert(schedules == 1 && uniwill_key_event_work.pending);
    remove_keyboard(); assert(removals == 1 && active_filter == EMPTY && drains == 1);
    remove_keyboard(); assert(removals == 1 && drains == 2);
    if (!strcmp(test, "reprobe")) {
        check_install_error(-EBUSY, FOREIGN);
        assert(uniwill_keyboard_probe(&platform) == 0);
        assert(uniwill_i8042_filter_installed && active_filter == OURS);
        send_complete_sequence(); assert(schedules == 2); remove_keyboard();
        assert(installations == 3 && removals == 2 && drains == 4);
    } else assert(!strcmp(test, "success"));
    return 0;
}
