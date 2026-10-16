/* SPDX-License-Identifier: GPL-2.0-only */
/* Test doubles surround verbatim driver functions; no physical EC access. */
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define LINUX_VERSION_CODE KERNEL_VERSION(7, 2, 9)
#define UW_EC_REG_CUSTOM_PROFILE 0x0727
#define UW_EC_REG_KBD_BL_STATUS 0x078c
#define UNIWILL_OSD_MODE_CHANGE_KEY_EVENT 0xb0
#define UNIWILL_OSD_DC_ADAPTER_CHANGE 0xab
#define UNIWILL_KEY_KBDILLUMTOGGLE 0xb9
#define UNIWILL_OSD_KB_LED_LEVEL0 0x3b
#define UNIWILL_OSD_KB_LED_LEVEL1 0x3c
#define UNIWILL_OSD_KB_LED_LEVEL2 0x3d
#define UNIWILL_OSD_KB_LED_LEVEL3 0x3e
#define UNIWILL_OSD_KB_LED_LEVEL4 0x3f
#define KEY_LEFTMETA 1
#define KEY_LEFTALT 2
#define KEY_F6 3
#define fallthrough __attribute__((fallthrough))
#define TUXEDO_DEBUG(...) do {} while (0)
#define pr_info(...) do {} while (0)
struct platform_device { int unused; };
struct uniwill_device_features_t {
    bool uniwill_custom_profile_mode_needed, uniwill_profile_v1;
};
static struct uniwill_device_features_t features;
static struct { void *input_device; } uniwill_keyboard_driver;
struct mutex { bool held; };
static struct mutex uniwill_fan_command_lock, uniwill_fan_state_lock;
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
#define lockdep_assert_held(m) assert((m)->held)
static bool uniwill_fan_work_paused, uniwill_fan_removed, fans_initialized;
static bool uniwill_pm_ready, uniwill_pm_suspended;
static bool universal_fan_manual_active;
static bool direct_fan_control_started, direct_fan_control_restart_needed, direct_fan_control_suspend;
static u8 direct_fan_control_current_value_fan0, direct_fan_control_current_value_fan1;
static u8 direct_fan_control_current_value_fan0_suspend_save, direct_fan_control_current_value_fan1_suspend_save;
static int direct_fan_control_restart_delayed_work;
static void cancel_delayed_work_sync(int *work) {
    assert(work == &direct_fan_control_restart_delayed_work);
    assert(uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
}
static u8 uniwill_touchp_toggle_seq_pos;
static bool uniwill_i8042_filter_installed;
/* @ADAPTER_DECL@ */
#define HAS_ADAPTER_GATE /* @HAS_ADAPTER_GATE@ */

static struct uniwill_device_features_t *uniwill_get_device_features(void) { return &features; }
struct operation { char kind; unsigned int address, value; };
static struct operation trace[64];
static int operations, custom_calls, reads, writes, sleeps;
static int fail_at, injected_errno, errors, reported_errno;
static int led_restores, priority_restores, profile_restores, resource_calls;
static unsigned int resource_mask;
static void resource(int index) {
    assert(!(resource_mask & (1U << index)));
    resource_mask |= 1U << index; ++resource_calls;
}
static u8 custom_value, uniwill_kbd_bl_enable_state_on_start;
static bool uw_lightbar_loaded;
static void record(char kind, unsigned int address, unsigned int value) {
    assert(operations < (int)(sizeof(trace) / sizeof(trace[0])));
    trace[operations++] = (struct operation){kind, address, value};
}
static int uniwill_read_ec_ram(unsigned int address, u8 *data) {
    record('r', address, 0);
    if (address == UW_EC_REG_CUSTOM_PROFILE) {
        ++reads; ++custom_calls;
        if (custom_calls == fail_at) return injected_errno;
        *data = custom_value;
    } else {
        *data = address == UW_EC_REG_KBD_BL_STATUS ? 2 : (u8)address;
    }
    return 0;
}
static int uniwill_write_ec_ram(unsigned int address, u8 data) {
    record('w', address, data);
    if (address == UW_EC_REG_CUSTOM_PROFILE) {
        ++writes; ++custom_calls;
        if (custom_calls == fail_at) return injected_errno;
        custom_value = data;
    }
    return 0;
}
static void msleep(int duration) { assert(duration == 50); ++sleeps; record('s', 0, duration); }
static void __attribute__((unused)) error_log(const char *format, ...) {
    va_list arguments;
    assert(strstr(format, "%d"));
    va_start(arguments, format); reported_errno = va_arg(arguments, int); va_end(arguments);
    ++errors; record('E', 0, 0);
}
#define TUXEDO_ERROR(...) error_log(__VA_ARGS__)
static void uniwill_leds_restore_state_extern(void) { ++led_restores; record('L', 0, 0); }
static void uw_charging_priority_write_state(void) { ++priority_restores; record('P', 0, 0); }
static void uw_charging_profile_write_state(void) { ++profile_restores; record('C', 0, 0); }
static void input_report_key(void *input, int key, int pressed) { (void)input; (void)key; (void)pressed; }
static void input_sync(void *input) { (void)input; }
static bool uniwill_leds_notify_brightness_change_extern(void) { return false; }
static bool sparse_keymap_report_known_event(void *input, u32 code, int value, bool down) {
    (void)input; (void)code; (void)value; (void)down; return true;
}
static void set_rom_id(void) {}
static int uniwill_write_kbd_bl_enable(u8 value) { assert(value == 1); resource(0); return 0; }
static void uniwill_leds_init(struct platform_device *dev) { (void)dev; resource(1); }
static int uw_lightbar_init(struct platform_device *dev) { (void)dev; resource(2); return 0; }
static void uw_charging_priority_init(struct platform_device *dev) { (void)dev; resource(3); }
static void uw_charging_profile_init(struct platform_device *dev) { (void)dev; resource(4); }
static void uw_ac_auto_boot_init(struct platform_device *dev) { (void)dev; resource(5); }
static void uw_usb_powershare_init(struct platform_device *dev) { (void)dev; resource(6); }
static void uw_mini_led_local_dimming_init(struct platform_device *dev) { (void)dev; resource(7); }
static void uw_show_hidden_bios_options(void) { resource(8); }
static void uw_battery_init(void) { resource(9); }
static void uniwill_i8042_filter(void) {}
static int i8042_install_filter(void (*filter)(void), void *context) {
    assert(filter == uniwill_i8042_filter && context == NULL);
    assert(uniwill_touchp_toggle_seq_pos == 0); resource(10); return 0;
}
/* @ACTUAL_FUNCTIONS@ */
static int invoke_helper(bool zero) { /* @INVOKE_HELPER@ */ }
static void reset(void) {
    memset(trace, 0, sizeof(trace));
    operations = custom_calls = reads = writes = sleeps = 0;
    errors = reported_errno = 0;
    led_restores = priority_restores = profile_restores = resource_calls = 0;
    resource_mask = 0;
    fail_at = 0; injected_errno = -ETIMEDOUT; custom_value = 0xa5;
    features.uniwill_custom_profile_mode_needed = true;
    features.uniwill_profile_v1 = false;
    uw_lightbar_loaded = false; uniwill_kbd_bl_enable_state_on_start = 0xff;
    uniwill_touchp_toggle_seq_pos = 5; uniwill_i8042_filter_installed = false;
    uniwill_pm_ready = uniwill_pm_suspended = false;
    uniwill_fan_removed = uniwill_fan_work_paused = false;
    direct_fan_control_started = direct_fan_control_restart_needed = direct_fan_control_suspend = false;
}
static void check_operation(int index, char kind, unsigned int address, unsigned int value) {
    assert(index < operations);
    assert(trace[index].kind == kind && trace[index].address == address && trace[index].value == value);
}
static void check_custom_trace(int offset, bool zero, u8 initial, int failure) {
    check_operation(offset, 'r', UW_EC_REG_CUSTOM_PROFILE, 0);
    if (failure == 1) { assert(reads == 1 && writes == 0); return; }
    int cursor = offset + 1;
    if (zero) {
        check_operation(cursor++, 'w', UW_EC_REG_CUSTOM_PROFILE, initial & ~0x40);
        if (failure == 2) { assert(reads == 1 && writes == 1); return; }
        check_operation(cursor++, 's', 0, 50);
    }
    check_operation(cursor, 'w', UW_EC_REG_CUSTOM_PROFILE, initial | 0x40);
    assert(reads == 1 && writes == (zero ? 2 : 1));
}
static void helper_case(bool zero, int failure, int error, u8 initial) {
    reset(); fail_at = failure; injected_errno = error; custom_value = initial;
    assert(invoke_helper(zero) == (failure ? error : 0));
    check_custom_trace(0, zero, initial, failure);
    assert(custom_calls == (failure ? failure : (zero ? 3 : 2)));
    assert(operations == custom_calls + sleeps);
    assert(sleeps == (zero && failure != 1 && failure != 2));
    assert(!errors);
    if (zero && failure == 3) assert(custom_value == (initial & ~0x40));
}
static void probe_case(int failure, int error, bool unsupported, bool curve) {
    reset(); fail_at = failure; injected_errno = error;
    features.uniwill_custom_profile_mode_needed = !unsupported;
    features.uniwill_profile_v1 = curve;
    struct platform_device dev = {0};
    u8 initial = custom_value;
    assert(uniwill_keyboard_probe(&dev) == 0);
    check_operation(0, 'w', 0x0751, 0);
    int offset = 1;
    if (curve) {
        for (int i = 0; i < 5; ++i) {
            check_operation(offset++, 'r', 0x0786 + i, 0);
            check_operation(offset++, 'w', 0x0743 + i, (u8)(0x0786 + i));
        }
    }
    if (unsupported) assert(!reads && !writes && !sleeps && !errors);
    else {
        check_custom_trace(offset, true, initial, failure);
        offset += custom_calls + sleeps;
        assert(errors == !!failure);
        if (failure) {
            assert(reported_errno == error); check_operation(offset++, 'E', 0, 0);
        }
    }
    check_operation(offset++, 'w', 0x0741, 1);
    check_operation(offset++, 'w', 0x044f, 0);
    check_operation(offset++, 'r', UW_EC_REG_KBD_BL_STATUS, 0);
    assert(operations == offset);
    assert(resource_calls == 11 && resource_mask == ((1U << 11) - 1) && uw_lightbar_loaded);
    assert(uniwill_kbd_bl_enable_state_on_start == 0);
    assert(uniwill_i8042_filter_installed && !uniwill_touchp_toggle_seq_pos);
    assert(!uniwill_fan_removed && !uniwill_fan_command_lock.held && !uniwill_fan_state_lock.held);
}
static void event_case(int failure, int error, bool unsupported) {
    reset();
    features.uniwill_custom_profile_mode_needed = !unsupported;
    struct platform_device dev = {0};
    assert(uniwill_keyboard_probe(&dev) == 0);
    if (HAS_ADAPTER_GATE) assert(uniwill_pm_ready && !uniwill_pm_suspended);
    /* Observe the cable event separately after the real probe opens its gate. */
    memset(trace, 0, sizeof(trace));
    operations = custom_calls = reads = writes = sleeps = errors = reported_errno = 0;
    led_restores = priority_restores = profile_restores = 0;
    fail_at = failure; injected_errno = error; custom_value = 0xa5;
    u8 initial = custom_value;
    uniwill_event_callb(UNIWILL_OSD_DC_ADAPTER_CHANGE);
    int offset = 0;
    if (unsupported) assert(!reads && !writes && !errors);
    else {
        check_custom_trace(offset, false, initial, failure);
        offset += custom_calls;
        assert(errors == !!failure);
        if (failure) {
            assert(reported_errno == error); check_operation(offset++, 'E', 0, 0);
        }
    }
    check_operation(offset++, 'L', 0, 0);
    check_operation(offset++, 's', 0, 50);
    check_operation(offset++, 'P', 0, 0);
    check_operation(offset++, 'C', 0, 0);
    assert(operations == offset && sleeps == 1);
    assert(led_restores == 1 && priority_restores == 1 && profile_restores == 1);
}
int main(int argc, char **argv) {
    assert(argc == 2);
    const char *name = argv[1];
    int errors_to_test[] = {-EIO, -ENODEV, -ETIMEDOUT};
    if (!strcmp(name, "unsupported_boot") || !strcmp(name, "unsupported_adapter")) {
        reset(); features.uniwill_custom_profile_mode_needed = false;
        assert(invoke_helper(!strcmp(name, "unsupported_boot")) == 0);
        assert(!operations && !reads && !writes && !sleeps);
    } else if (!strcmp(name, "boot_bytes") || !strcmp(name, "adapter_bytes")) {
        bool zero = !strcmp(name, "boot_bytes");
        for (int value = 0; value <= 255; ++value) {
            reset(); custom_value = value;
            assert(invoke_helper(zero) == 0);
            check_custom_trace(0, zero, value, 0);
            assert(custom_value == (value | 0x40));
            assert(operations == (zero ? 4 : 2) && sleeps == zero);
        }
    } else if (!strcmp(name, "probe_unsupported")) probe_case(0, 0, true, false);
    else if (!strcmp(name, "probe_success")) probe_case(0, 0, false, false);
    else if (!strcmp(name, "probe_curve")) probe_case(0, 0, false, true);
    else if (!strcmp(name, "event_unsupported")) event_case(0, 0, true);
    else if (!strcmp(name, "event_success")) event_case(0, 0, false);
    else if (!strcmp(name, "probe_failures")) {
        for (int step = 1; step <= 3; ++step)
            for (unsigned int i = 0; i < sizeof(errors_to_test)/sizeof(errors_to_test[0]); ++i)
                probe_case(step, errors_to_test[i], false, false);
    } else if (!strcmp(name, "event_failures")) {
        for (int step = 1; step <= 2; ++step)
            for (unsigned int i = 0; i < sizeof(errors_to_test)/sizeof(errors_to_test[0]); ++i)
                event_case(step, errors_to_test[i], false);
    } else {
        bool zero = !strncmp(name, "boot_", 5);
        int failure = strstr(name, "read_") ? 1 : (zero && strstr(name, "reset_") ? 2 : (zero ? 3 : 2));
        for (unsigned int i = 0; i < sizeof(errors_to_test)/sizeof(errors_to_test[0]); ++i)
            for (int initial = 0xa5; initial <= 0xe5; initial += 0x40)
                helper_case(zero, failure, errors_to_test[i], initial);
    }
    return 0;
}
