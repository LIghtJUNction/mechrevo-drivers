/* Actual outer binding functions and initial fan-start path, mocked kernel APIs. */
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
typedef uint8_t u8;
typedef uint32_t u32;
struct device { struct device *parent; };
struct platform_device { struct device dev; };
struct input_dev { const char *name, *phys; struct { int bustype; } id; struct device dev; };
struct key_entry { int unused; };
struct platform_driver { struct { const char *name; } driver; };
struct tuxedo_keyboard_driver {
    struct platform_driver *platform_driver;
    int (*probe)(struct platform_device *);
    struct key_entry *key_map;
    struct input_dev *input_device;
    bool (*fn_lock_available)(void);
    void (*prepare_remove)(void);
};
struct uniwill_interface_t { const char *string_id; void (*event_callb)(u32); };
struct wmi_device { int unused; };
struct uniwill_device_features_t { bool uniwill_profile_v1; };
static struct uniwill_device_features_t features;
static struct uniwill_device_features_t *uniwill_get_device_features(void) { return &features; }
static void set_rom_id(void) {}
struct mutex { bool held; };
#define DEFINE_MUTEX(n) struct mutex n
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
#define lockdep_assert_held(m) assert((m)->held)
static DEFINE_MUTEX(tuxedo_keyboard_init_driver_lock);
static DEFINE_MUTEX(uniwill_interface_modification_lock);
static DEFINE_MUTEX(uniwill_fan_command_lock);
static DEFINE_MUTEX(uniwill_fan_state_lock);
static bool uniwill_fan_work_paused, uniwill_fan_removed, fans_initialized;
static u8 uniwill_kbd_bl_enable_state_on_start;
static bool direct_fan_control_started, direct_fan_control_restart_needed, direct_fan_control_suspend;
static u8 direct_fan_control_current_value_fan0, direct_fan_control_current_value_fan1;
static u8 direct_fan_control_current_value_fan0_suspend_save, direct_fan_control_current_value_fan1_suspend_save;
static int direct_fan_control_restart_delayed_work;
static void cancel_delayed_work_sync(int *w) { (void)w; }
static int ec_error, ec_writes;
static int uniwill_write_ec_ram(unsigned int address, u8 value) {
    assert(address == 0x0751 && value == 0); ++ec_writes; return ec_error;
}
#define ERR_PTR(e) ((void *)(intptr_t)(e))
#define PTR_ERR(p) ((int)(intptr_t)(p))
#define IS_ERR_OR_NULL(p) (!(p) || (uintptr_t)(p) >= (uintptr_t)-4095)
#define unlikely(v) (v)
#define EXPORT_SYMBOL(n)
#define TUXEDO_DEBUG(...) do {} while (0)
#define TUXEDO_ERROR(...) do {} while (0)
#define pr_debug(...) do {} while (0)
#define pr_err(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define DRIVER_NAME "tuxedo_keyboard"
#define BUS_HOST 1
#define UNIWILL_INTERFACE_WMI_STRID "uniwill_wmi"
#define UNIWILL_WMI_EVENT_GUID_0 "0"
#define UNIWILL_WMI_EVENT_GUID_1 "1"
#define UNIWILL_WMI_EVENT_GUID_2 "2"
#define UNIWILL_WMI_MGMT_GUID_BA "a"
#define UNIWILL_WMI_MGMT_GUID_BB "b"
#define UNIWILL_WMI_MGMT_GUID_BC "c"
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
static struct platform_device platform;
static struct input_dev input;
static struct platform_device *tuxedo_platform_device;
static struct input_dev *tuxedo_input_device;
static struct tuxedo_keyboard_driver *current_driver;
static struct { struct uniwill_interface_t *wmi; } uniwill_interfaces;
static struct uniwill_interface_t uniwill_wmi_interface = { .string_id = UNIWILL_INTERFACE_WMI_STRID };
static struct platform_driver platform_driver_uniwill = { .driver = { DRIVER_NAME } };
static struct platform_driver platform_driver_other = { .driver = { DRIVER_NAME } };
static struct key_entry keys;
static bool fn_available(void) { return false; }
static int uniwill_keyboard_probe(struct platform_device *dev);
static void uniwill_event_callb(u32 code);
static struct tuxedo_keyboard_driver uniwill_keyboard_driver = {
    &platform_driver_uniwill, uniwill_keyboard_probe, &keys, NULL, fn_available, NULL
};
static struct tuxedo_keyboard_driver other_driver = {
    &platform_driver_other, uniwill_keyboard_probe, &keys, NULL, fn_available, NULL
};
static int keymap_error, input_error, input_frees, input_unregs;
static int bundles, platform_unregs, driver_unregs, key_events;
static bool missing_guid, check_events, input_freed, allocation_failure;
static int dev_attr_fn_lock;
static struct input_dev *input_allocate_device(void) { input_freed = false; return allocation_failure ? NULL : &input; }
static int sparse_keymap_setup(struct input_dev *d, const struct key_entry *k, void *unused) {
    (void)d; (void)k; (void)unused; return keymap_error;
}
static int input_register_device(struct input_dev *d) { assert(d == &input); return input_error; }
static void input_free_device(struct input_dev *d) { assert(d == &input && !input_freed); ++input_frees; input_freed = true; }
static void input_unregister_device(struct input_dev *d) { assert(d == &input && !input_freed); ++input_unregs; input_freed = true; }
static int device_create_file(struct device *d, int *a) { (void)d; (void)a; return 0; }
static void device_remove_file(struct device *d, int *a) { assert(d == &platform.dev); (void)a; }
static struct platform_device *platform_create_bundle(struct platform_driver *d,
        int (*probe)(struct platform_device *), void *r, int nr, void *data, int size) {
    (void)d; (void)r; (void)nr; (void)data; (void)size; ++bundles;
    if (check_events) assert(!uniwill_wmi_interface.event_callb);
    int status = probe(&platform);
    return status ? ERR_PTR(status) : &platform;
}
static void platform_device_unregister(struct platform_device *d) {
    assert(d == &platform); ++platform_unregs;
    if (check_events) assert(!uniwill_wmi_interface.event_callb);
}
static void platform_driver_unregister(struct platform_driver *d) { assert(d); ++driver_unregs; }
static void input_report_key(struct input_dev *d, int key, int down) {
    assert(d == &input && !input_freed); (void)key; (void)down; ++key_events;
}
static void input_sync(struct input_dev *d) { assert(d == &input && !input_freed); }
static int uniwill_set_custom_profile_mode(bool b) { (void)b; return 0; }
static void uniwill_leds_restore_state_extern(void) {}
static void msleep(int n) { (void)n; }
static void uw_charging_priority_write_state(void) {}
static void uw_charging_profile_write_state(void) {}
static bool uniwill_leds_notify_brightness_change_extern(void) { return false; }
static bool sparse_keymap_report_known_event(struct input_dev *d, u32 c, int n, bool b) { (void)d; (void)c; (void)n; (void)b; return true; }
static bool wmi_has_guid(const char *g) { (void)g; return !missing_guid; }
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#pragma GCC diagnostic ignored "-Wunused-variable"
#pragma GCC diagnostic ignored "-Wunused-but-set-variable"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop
static void bind_ok(void) {
    assert(uniwill_wmi_probe(NULL, NULL) == 0);
    assert(current_driver == &uniwill_keyboard_driver && tuxedo_platform_device == &platform);
    assert(uniwill_interfaces.wmi == &uniwill_wmi_interface);
    assert(uniwill_wmi_interface.event_callb == uniwill_event_callb);
}
int main(int argc, char **argv) {
    assert(argc == 2);
    const char *name = argv[1];
    if (!strcmp(name, "probe_failure")) {
        ec_error = -ETIMEDOUT;
        assert(uniwill_wmi_probe(NULL, NULL) == -ETIMEDOUT);
        assert(!tuxedo_platform_device && !current_driver && !tuxedo_input_device);
        assert(!uniwill_interfaces.wmi && !uniwill_wmi_interface.event_callb);
        assert(uniwill_fan_removed && ec_writes == 1);
    } else if (!strcmp(name, "failed_remove")) {
        ec_error = -ETIMEDOUT; (void)uniwill_wmi_probe(NULL, NULL);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0);
        tuxedo_keyboard_remove_driver(&uniwill_keyboard_driver);
        tuxedo_keyboard_remove_driver(NULL);
        assert(!current_driver && !tuxedo_platform_device && platform_unregs == 0 && driver_unregs == 0);
    } else if (!strcmp(name, "retry")) {
        ec_error = -EIO; (void)uniwill_wmi_probe(NULL, NULL); ec_error = 0;
        bind_ok(); assert(!uniwill_fan_removed && bundles == 2);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0);
        assert(!current_driver && !tuxedo_platform_device && !tuxedo_input_device);
        bind_ok(); assert(bundles == 3);
    } else if (!strcmp(name, "same_driver")) {
        bind_ok(); assert(tuxedo_keyboard_init_driver(&uniwill_keyboard_driver) == &platform);
        assert(bundles == 1);
    } else if (!strcmp(name, "other_driver")) {
        bind_ok(); assert(PTR_ERR(tuxedo_keyboard_init_driver(&other_driver)) == -EBUSY);
        tuxedo_keyboard_remove_driver(&other_driver);
        assert(current_driver == &uniwill_keyboard_driver && !platform_unregs);
    } else if (!strcmp(name, "wrong_interface") || !strcmp(name, "duplicate")) {
        bind_ok(); struct uniwill_interface_t other = { .string_id = UNIWILL_INTERFACE_WMI_STRID };
        if (!strcmp(name, "duplicate")) assert(uniwill_add_interface(&other) == -EBUSY);
        else assert(uniwill_remove_interface(&other) == 0);
        assert(current_driver == &uniwill_keyboard_driver && !platform_unregs);
        assert(uniwill_interfaces.wmi == &uniwill_wmi_interface && uniwill_wmi_interface.event_callb);
    } else if (!strcmp(name, "keymap_failure") || !strcmp(name, "input_failure")) {
        if (!strcmp(name, "keymap_failure")) keymap_error = -ENOMEM;
        else input_error = -EIO;
        bind_ok(); assert(input_frees == 1 && !tuxedo_input_device && !uniwill_keyboard_driver.input_device);
        uniwill_event_callb(UNIWILL_OSD_MODE_CHANGE_KEY_EVENT);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0 && input_unregs == 0);
    } else if (!strcmp(name, "allocation_failure")) {
        allocation_failure = true; bind_ok();
        assert(!tuxedo_input_device && !uniwill_keyboard_driver.input_device && input_frees == 0);
        uniwill_event_callb(UNIWILL_OSD_MODE_CHANGE_KEY_EVENT);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0 && input_unregs == 0);
    } else if (!strcmp(name, "mode_no_input")) {
        uniwill_event_callb(UNIWILL_OSD_MODE_CHANGE_KEY_EVENT); assert(key_events == 0);
    } else if (!strcmp(name, "event_lifecycle")) {
        check_events = true; bind_ok();
        uniwill_wmi_interface.event_callb(UNIWILL_OSD_MODE_CHANGE_KEY_EVENT); assert(key_events == 6);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0);
        assert(!uniwill_keyboard_driver.input_device && !uniwill_wmi_interface.event_callb);
        assert(uniwill_remove_interface(&uniwill_wmi_interface) == 0);
        assert(platform_unregs == 1 && driver_unregs == 1 && input_unregs == 1);
    } else if (!strcmp(name, "missing_guid")) {
        missing_guid = true; assert(uniwill_wmi_probe(NULL, NULL) == -ENODEV);
        assert(bundles == 0 && !uniwill_interfaces.wmi);
    } else return 2;
    return 0;
}
