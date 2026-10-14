/* Complete real probe/filter/remove; only external kernel services are mocked. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define KERNEL_VERSION(a,b,c) (((a)<<16)|((b)<<8)|(c))
#define __always_unused __attribute__((unused))
#define unlikely(v) (v)
#define ARRAY_SIZE(a) (sizeof(a)/sizeof((a)[0]))
#define I8042_STR_AUXDATA 0x20
#define UW_EC_REG_KBD_BL_STATUS 0x078c
#define TUXEDO_ERROR(...) do {} while (0)
#define pr_info(...) do {} while (0)
struct mutex { bool held; };
static struct mutex uniwill_fan_command_lock, uniwill_fan_state_lock;
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
struct work_struct { bool pending; };
static struct work_struct uniwill_key_event_work;
struct device { struct { int unused; } kobj; };
struct platform_device { struct device dev; };
struct serio { int unused; };
struct uniwill_device_features_t { bool uniwill_profile_v1; };
static struct platform_device platform;
static struct uniwill_device_features_t features;
static bool uw_lightbar_loaded, uw_charging_prio_loaded, uw_charging_profile_loaded;
static bool uniwill_fan_removed;
static int uw_charging_prio_attr_group, uw_charging_profile_attr_group;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
enum filter_slot { EMPTY, OURS, FOREIGN };
static enum filter_slot active_filter;
static int install_errno, start_errno, installations, removals, drains, schedules, tail_initializations;
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
static int uw_fan_control_start(void) { uniwill_fan_removed = start_errno != 0; return start_errno; }
static int uniwill_read_ec_ram(unsigned int address, u8 *data) { (void)address; *data = 0; return 0; }
static int uniwill_write_ec_ram(unsigned int address, u8 data) { (void)address; (void)data; return 0; }
static int uniwill_set_custom_profile_mode(bool enabled) { assert(enabled); ++tail_initializations; return 0; }
static void uniwill_leds_init(struct platform_device *dev) { assert(dev == &platform); }
static void uniwill_write_kbd_bl_enable(u8 enabled) { assert(enabled <= 1); }
static int uw_lightbar_init(struct platform_device *dev) { assert(dev == &platform); return -ENODEV; }
#define INIT_STUB(name) static void name(struct platform_device *dev) { assert(dev == &platform); }
INIT_STUB(uw_charging_priority_init)
INIT_STUB(uw_charging_profile_init)
INIT_STUB(uw_ac_auto_boot_init)
INIT_STUB(uw_usb_powershare_init)
INIT_STUB(uw_mini_led_local_dimming_init)
static void uw_show_hidden_bios_options(void) {}
static void uw_battery_init(void) {}
static void uw_battery_uninit(void) {}
static void sysfs_remove_group(void *kobj, void *group) { (void)kobj; (void)group; }
static void uniwill_leds_remove(struct platform_device *dev) { assert(dev == &platform); }
static void uw_lightbar_remove(struct platform_device *dev) { assert(dev == &platform); }
static void uw_fan_quiesce_locked(void) {
    assert(uniwill_fan_command_lock.held && uniwill_fan_state_lock.held);
}
/* @DRIVER_FUNCTIONS@ */
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
int main(int argc, char **argv) {
    assert(argc == 2); const char *test = argv[1];
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
