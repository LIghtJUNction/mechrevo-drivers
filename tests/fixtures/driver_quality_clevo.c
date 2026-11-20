#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
typedef uint8_t u8;
typedef uint32_t u32;
typedef uint64_t u64;
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define IS_ERR_OR_NULL(p) (!(p) || (uintptr_t)(p) >= (uintptr_t)-4095)
#define ZERO_OR_NULL_PTR(p) (!(p))
#define ERR_PTR(e) ((void *)(intptr_t)(e))
#define PTR_ERR(p) ((int)(intptr_t)(p))
#define __always_unused
#define EXPORT_SYMBOL(n)
#define pr_err(...) do {} while (0)
#define pr_notice(...) do {} while (0)
#define pr_debug(...) do {} while (0)
#define TUXEDO_INFO(...) do {} while (0)
#define DMI_PRODUCT_SKU 1
#define DMI_BOARD_NAME 2
#define DMI_PRODUCT_NAME 3
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_BUFFER 3
#define LED_FUNCTION_KBD_BACKLIGHT "kbd_backlight"
#define LED_BRIGHT_HW_CHANGED 1
#define LED_COLOR_ID_RED 0
#define LED_COLOR_ID_GREEN 1
#define LED_COLOR_ID_BLUE 2
#define KEY_LEFTMETA 1
#define KEY_LEFTALT 2
#define KEY_F6 3
#define clamp_t(t, v, lo, hi) ((t)(v) < (lo) ? (lo) : (t)(v) > (hi) ? (hi) : (t)(v))
/* @CONSTANTS@ */
/* @LED_ENUM@ */
enum led_brightness { LED_OFF = 0, LED_FULL = 255 };
struct device { int unused; };
struct device_attribute { int unused; };
struct platform_device { struct device dev; };
struct input_dev { int unused; };
struct led_classdev {
    const char *name;
    enum led_brightness brightness;
    unsigned int max_brightness, flags;
    void (*brightness_set)(struct led_classdev *, enum led_brightness);
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
};
struct mc_subled { unsigned int color_index, brightness, intensity; u32 channel; };
struct led_classdev_mc { struct led_classdev led_cdev; int num_colors; struct mc_subled *subled_info; };
#define lcdev_to_mccdev(d) ((struct led_classdev_mc *)(d))
struct kernel_param { void *arg; };
struct kernel_param_ops { int (*set)(const char *, const struct kernel_param *); int (*get)(char *, const struct kernel_param *); };
union acpi_object {
    u32 type;
    struct { u32 type; u64 value; } integer;
    struct { u32 type, length; u8 *pointer; } buffer;
};
struct clevo_interface_t {
    char *string_id;
    void (*event_callb)(u32);
    int (*method_call)(u8, u32, union acpi_object **);
    int (*method_call_pkgbuf)(u8, u8 *, u32, union acpi_object **);
};
static struct { struct clevo_interface_t *wmi, *acpi; } clevo_interfaces;
static struct clevo_interface_t *active_clevo_interface;
static struct { struct input_dev *input_device; } clevo_keyboard_driver;
static bool leds_initialized;
static enum clevo_kb_backlight_types clevo_kb_backlight_type;
static int clevo_leds_set_brightness(struct led_classdev *, enum led_brightness);
static int clevo_leds_set_brightness_mc(struct led_classdev *, enum led_brightness);
static int kbd_backlight_mode_id_validator(const char *, const struct kernel_param *);
static int param_get_int(char *, const struct kernel_param *);
static int param_get_byte(char *, const struct kernel_param *);
/* @DECLARATIONS@ */
static int dmi_match(int field, const char *value) { return 0; }
static bool dmi_string_in(int field, const char *value) { return false; }
static void msleep(unsigned int n) {}
static int kstrtoint(const char *s, int base, int *out)
{
    char *end; long n;
    errno = 0; n = strtol(s, &end, base);
    if (s == end || errno || n < INT_MIN || n > INT_MAX) return -EINVAL;
    if (*end && strcmp(end, "\n")) return -EINVAL;
    *out = n; return 0;
}
static int kstrtouint(const char *s, int base, unsigned int *out)
{
    char *end; unsigned long n;
    if (*s == '-') return -EINVAL;
    errno = 0; n = strtoul(s, &end, base);
    if (s == end || errno || n > UINT_MAX) return -EINVAL;
    if (*end && strcmp(end, "\n")) return -EINVAL;
    *out = n; return 0;
}
static int param_set_int(const char *s, const struct kernel_param *p)
{ return kstrtoint(s, 0, p->arg); }
static int param_get_int(char *s, const struct kernel_param *p)
{ return sprintf(s, "%d\n", *(int *)p->arg); }
static int param_set_byte(const char *s, const struct kernel_param *p)
{
    unsigned int value; int status = kstrtouint(s, 0, &value);
    if (status || value > 255) return -EINVAL;
    *(u8 *)p->arg = value; return 0;
}
static int param_get_byte(char *s, const struct kernel_param *p)
{ return sprintf(s, "%u\n", *(u8 *)p->arg); }
static void *live[64];
static int allocations, frees, object_status, object_calls;
static union acpi_object *response;
static int spec_calls, spec_errors[3], spec_type[3] = { 3, 3, 3 };
static unsigned int spec_len[3] = { 16, 16, 16 };
static u8 spec_value[3] = { 1, 1, 1 };
static bool spec_null;
static union acpi_object *object(u32 type, unsigned int length)
{
    union acpi_object *o = calloc(1, sizeof(*o) + length);
    assert(o && allocations < 64);
    live[allocations++] = o;
    o->buffer.type = type;
    o->buffer.length = length;
    o->buffer.pointer = (u8 *)(o + 1);
    return o;
}
static void free_object(void *p)
{
    if (!p) return;
    int i;
    for (i = 0; i < allocations && live[i] != p; ++i) {}
    assert(i < allocations);
    live[i] = NULL; ++frees; free(p);
}
#define ACPI_FREE(p) free_object(p)
static int clevo_evaluate_method2(u8 cmd, u32 arg, union acpi_object **out)
{
    if (cmd == CLEVO_CMD_GET_SPECS) {
        int i = spec_calls++;
        assert(i < 3);
        if (spec_errors[i]) return spec_errors[i];
        *out = object(spec_type[i], spec_len[i]);
        if (spec_len[i] > 15) (*out)->buffer.pointer[15] = spec_value[i];
        if (spec_null) (*out)->buffer.pointer = NULL;
        return 0;
    }
    assert(cmd == 4 && arg == 0x18);
    ++object_calls;
    if (object_status) return object_status;
    *out = response; response = NULL;
    assert(*out);
    return 0;
}
static int method_calls, method_status, method_fail_at, read_status;
static u32 method_result, last_method_arg, read_brightness;
static int features1_status, features2_status;
static u32 features1, features2;
static int clevo_evaluate_method(u8 cmd, u32 arg, u32 *out)
{
    if (cmd == CLEVO_CMD_GET_BIOS_FEATURES_1) {
        if (!features1_status && out) *out = features1;
        return features1_status;
    }
    if (cmd == CLEVO_CMD_GET_BIOS_FEATURES_2) {
        if (!features2_status && out) *out = features2;
        return features2_status;
    }
    if (cmd == CLEVO_CMD_GET_KB_WHITE_LEDS) {
        if (!read_status && out) *out = read_brightness;
        return read_status;
    }
    ++method_calls; last_method_arg = arg;
    if (method_status && (!method_fail_at || method_calls == method_fail_at)) return method_status;
    if (out) *out = method_result;
    return 0;
}
static int package_calls, package_status;
static u8 package_arg[4];
static int clevo_evaluate_method_pkgbuf(u8 cmd, u8 *arg, u32 length, union acpi_object **out)
{
    assert(cmd == 4 && length == sizeof(package_arg));
    memcpy(package_arg, arg, length); ++package_calls;
    if (package_status) return package_status;
    *out = object(ACPI_TYPE_INTEGER, 0);
    return 0;
}
static int registrations, unregistrations, notifications;
static int led_classdev_register(struct device *d, struct led_classdev *led)
{
    assert(led->brightness_set_blocking && !led->brightness_set);
    ++registrations; return 0;
}
static void led_classdev_unregister(struct led_classdev *led) { ++unregistrations; }
static int devm_led_classdev_multicolor_register(struct device *d, struct led_classdev_mc *led)
{ return led_classdev_register(d, &led->led_cdev); }
static void devm_led_classdev_multicolor_unregister(struct device *d, struct led_classdev_mc *led)
{ ++unregistrations; }
static void led_classdev_notify_brightness_hw_changed(struct led_classdev *led, u32 value)
{ assert(leds_initialized && value == led->brightness); ++notifications; }
static void clevo_leds_set_brightness_extern(enum led_brightness brightness);
static void clevo_leds_set_color_extern(u32 color);
/* @FUNCTION clevo_leds.h color_scaling@ */
/* @FUNCTION clevo_leds.h clevo_evaluate_set_white_brightness@ */
/* @FUNCTION clevo_leds.h clevo_evaluate_set_rgb_brightness@ */
/* @FUNCTION clevo_leds.h clevo_evaluate_set_rgb_color@ */
/* @FUNCTION clevo_leds.h clevo_evaluate_set_keyboard_status@ */
/* @FUNCTION clevo_leds.h clevo_leds_set_brightness@ */
/* @FUNCTION clevo_leds.h clevo_leds_set_brightness_mc@ */
/* @FUNCTION clevo_leds.h clevo_leds_set_brightness_extern@ */
/* @FUNCTION clevo_leds.h clevo_leds_set_color_extern@ */
/* @FUNCTION clevo_leds.h clevo_leds_init@ */
/* @FUNCTION clevo_leds.h clevo_leds_notify_brightness_change_extern@ */
/* @FUNCTION clevo_keyboard.h clevo_acpi_fn_get@ */
/* @FUNCTION clevo_keyboard.h clevo_acpi_fn_lock_set@ */
/* @FUNCTION clevo_keyboard.h clevo_fn_lock_available@ */
/* @FUNCTION clevo_keyboard.h set_kbd_backlight_mode@ */
/* @FUNCTION clevo_keyboard.h set_kbd_backlight_mode_fs@ */
/* @FUNCTION clevo_keyboard.h kbd_backlight_mode_id_validator@ */
/* @FUNCTION clevo_keyboard.h clevo_legacy_flexicharger_read@ */
/* @FUNCTION clevo_keyboard.h clevo_has_legacy_flexicharger@ */
static bool charge_control_registered;
static int battery_hook, hook_registered, hook_unregistered;
static void battery_hook_register(int *h) { assert(h == &battery_hook); ++hook_registered; }
static void battery_hook_unregister(int *h) { assert(h == &battery_hook); ++hook_unregistered; }
/* @FUNCTION clevo_keyboard.h clevo_flexicharger_init@ */
/* @FUNCTION clevo_keyboard.h clevo_flexicharger_remove@ */
struct mutex { bool held; };
static struct mutex clevo_keyboard_interface_modification_lock;
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
static int binding_status, bindings, removals, event_enables;
static struct platform_device platform;
static struct platform_device *tuxedo_keyboard_init_driver(void *driver)
{ ++bindings; return binding_status ? ERR_PTR(binding_status) : &platform; }
static void tuxedo_keyboard_remove_driver(void *driver) { ++removals; }
static void clevo_keyboard_event_callb(u32 event) {}
/* @FUNCTION clevo_keyboard.h clevo_keyboard_add_interface@ */
/* @FUNCTION clevo_keyboard.h clevo_keyboard_remove_interface@ */
static int key_events;
static void input_report_key(struct input_dev *d, int key, int value) { assert(d); ++key_events; }
static void input_sync(struct input_dev *d) { assert(d); }
/* @FUNCTION clevo_keyboard.h clevo_send_cc_combo@ */
static int interface_method(u8 cmd, u32 arg, union acpi_object **out)
{ assert(cmd == CLEVO_CMD_SET_EVENTS_ENABLED); ++event_enables; return 0; }
static void fn_response(u8 flag, u8 first, u8 second)
{
    response = object(ACPI_TYPE_BUFFER, 3);
    response->buffer.pointer[0] = flag;
    response->buffer.pointer[1] = first;
    response->buffer.pointer[2] = second;
}

int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *name = argv[1];
    struct clevo_interface_t wmi = { .string_id = CLEVO_INTERFACE_WMI_STRID, .method_call = interface_method };
    struct clevo_interface_t acpi = { .string_id = CLEVO_INTERFACE_ACPI_STRID, .method_call = interface_method };
    if (!strcmp(name, "fn_get_valid")) {
        for (u8 flag = 1; flag <= 2; ++flag) {
            u8 on = 9, first = 9, second = 9;
            fn_response(flag, 0x81, 0x42);
            assert(!clevo_acpi_fn_get(&on, &first, &second));
            assert(on == (flag == 1) && first == 0x81 && second == 0x42);
        }
    } else if (!strcmp(name, "fn_get_invalid")) {
        for (int variant = 0; variant < 7; ++variant) {
            u8 on = 9, first = 9, second = 9;
            fn_response(1, 0x81, 0x42);
            if (variant == 0) response->type = ACPI_TYPE_INTEGER;
            else if (variant < 4) response->buffer.length = variant - 1;
            else if (variant == 4) response->buffer.pointer = NULL;
            else response->buffer.pointer[0] = variant == 5 ? 0 : 3;
            assert(clevo_acpi_fn_get(&on, &first, &second) == -ENODATA);
            assert(on == 9 && first == 9 && second == 9 && allocations == frees);
        }
    } else if (!strcmp(name, "fn_get_failure")) {
        u8 on = 9, first = 9, second = 9;
        object_status = -ETIMEDOUT;
        assert(clevo_acpi_fn_get(&on, &first, &second) == -ETIMEDOUT);
        assert(on == 9 && first == 9 && second == 9 && !allocations);
    } else if (!strcmp(name, "fn_set_invalid_read")) {
        response = object(ACPI_TYPE_BUFFER, 2);
        assert(clevo_acpi_fn_lock_set(1) == -ENODATA && !package_calls);
    } else if (!strcmp(name, "fn_set_success")) {
        fn_response(2, 0x81, 0x42);
        assert(!clevo_acpi_fn_lock_set(1) && package_calls == 1);
        assert(package_arg[0] == 0x19 && package_arg[1] == 1 && package_arg[2] == 0x81 && package_arg[3] == 0x42);
    } else if (!strcmp(name, "fn_availability")) {
        assert(!clevo_fn_lock_available());
        active_clevo_interface = &wmi; assert(!clevo_fn_lock_available() && !object_calls);
        active_clevo_interface = &acpi;
        response = object(ACPI_TYPE_INTEGER, 0); assert(!clevo_fn_lock_available());
        fn_response(1, 1, 2); assert(clevo_fn_lock_available());
    } else if (!strcmp(name, "parameter_width")) {
        struct { u8 before, value, after[3]; } guarded = { 0xa5, 0, { 0xaa, 0xbb, 0xcc } };
        struct kernel_param param = { .arg = &guarded.value };
        char output[32];
        assert(!param_ops_mode_ops.set("7", &param));
        assert(guarded.before == 0xa5 && guarded.value == 7 && guarded.after[0] == 0xaa && guarded.after[1] == 0xbb && guarded.after[2] == 0xcc);
        assert(param_ops_mode_ops.get(output, &param) == 2 && !strcmp(output, "7\n"));
        assert(param_ops_mode_ops.set("8", &param) == -EINVAL && guarded.value == 7);
    } else if (!strcmp(name, "mode_invalid")) {
        const char *values[] = { "8", "255", "256", "4294967295", "-1", "invalid" };
        for (size_t i = 0; i < ARRAY_SIZE(values); ++i)
            assert(set_kbd_backlight_mode_fs(NULL, NULL, values[i], strlen(values[i])) < 0);
        assert(!method_calls);
    } else if (!strcmp(name, "mode_failure")) {
        kbd_led_state.mode = 2; method_status = -EIO;
        assert(set_kbd_backlight_mode_fs(NULL, NULL, "3", 1) == -EIO);
        assert(kbd_led_state.mode == 2);
    } else if (!strcmp(name, "mode_success")) {
        assert(set_kbd_backlight_mode_fs(NULL, NULL, "7\n", 2) == 2 && kbd_led_state.mode == 7);
        assert(last_method_arg == kbd_backlight_modes[7].value);
    } else if (!strcmp(name, "legacy_read_failure")) {
        u8 start = 1, end = 2, enabled = 3;
        method_status = -ETIMEDOUT;
        assert(clevo_legacy_flexicharger_read(&start, &end, &enabled) == -ETIMEDOUT);
        assert(start == 1 && end == 2 && enabled == 3);
    } else if (!strcmp(name, "legacy_probe_failure")) {
        bool supported = true;
        method_status = -EIO;
        assert(clevo_has_legacy_flexicharger(&supported) == -EIO && !supported);
        method_calls = 0; method_fail_at = 2; method_result = 0x76; supported = true;
        assert(clevo_has_legacy_flexicharger(&supported) == -EIO && !supported);
    } else if (!strncmp(name, "led_spec_", 9)) {
        if (!strcmp(name, "led_spec_short")) {
            spec_len[0] = spec_len[1] = spec_len[2] = 1;
            features1 = CLEVO_CMD_GET_BIOS_FEATURES_1_SUB_WHITE_ONLY_KB;
        } else if (!strcmp(name, "led_spec_retry")) {
            spec_value[0] = spec_value[1] = 0;
        } else if (!strcmp(name, "led_spec_reset")) {
            clevo_kb_backlight_type = CLEVO_KB_BACKLIGHT_TYPE_3_ZONE_RGB;
            clevo_led_cdev.max_brightness = 5;
            spec_errors[0] = spec_errors[1] = spec_errors[2] = -EIO;
            features1_status = -EIO;
        } else assert(!strcmp(name, "led_spec_success"));
        assert(!clevo_leds_init(&platform));
        assert(allocations == frees);
        if (!strcmp(name, "led_spec_reset")) {
            assert(clevo_kb_backlight_type == CLEVO_KB_BACKLIGHT_TYPE_NONE && registrations == 0);
            assert(clevo_led_cdev.max_brightness == CLEVO_KBD_BRIGHTNESS_WHITE_MAX);
        } else {
            assert(clevo_kb_backlight_type == CLEVO_KB_BACKLIGHT_TYPE_FIXED_COLOR && registrations == 1);
        }
        if (!strcmp(name, "led_spec_retry")) assert(spec_calls == 3);
    } else if (!strcmp(name, "led_brightness_error")) {
        assert(clevo_led_cdev.brightness_set_blocking && !clevo_led_cdev.brightness_set);
        clevo_led_cdev.brightness = 1; method_status = -ETIMEDOUT;
        assert(clevo_led_cdev.brightness_set_blocking(&clevo_led_cdev, 2) == -ETIMEDOUT);
        assert(clevo_led_cdev.brightness == 1);
    } else if (!strcmp(name, "led_rgb_error")) {
        struct led_classdev *led = &clevo_mcled_cdevs[0].led_cdev;
        assert(led->brightness_set_blocking && !led->brightness_set);
        method_status = -EIO;
        assert(led->brightness_set_blocking(led, 2) == -EIO && method_calls == 1);
        method_calls = 0; method_fail_at = 2;
        assert(led->brightness_set_blocking(led, 2) == -EIO && method_calls == 2);
    } else if (!strcmp(name, "led_notify_failure")) {
        clevo_kb_backlight_type = CLEVO_KB_BACKLIGHT_TYPE_FIXED_COLOR;
        clevo_led_cdev.brightness = 1;
        read_brightness = 2; clevo_leds_notify_brightness_change_extern(); assert(!notifications);
        leds_initialized = true; read_status = -EIO;
        clevo_leds_notify_brightness_change_extern();
        read_status = 0; read_brightness = 255; clevo_leds_notify_brightness_change_extern();
        assert(!notifications && clevo_led_cdev.brightness == 1);
    } else if (!strcmp(name, "led_notify_success")) {
        leds_initialized = true; clevo_kb_backlight_type = CLEVO_KB_BACKLIGHT_TYPE_FIXED_COLOR;
        read_brightness = 2; clevo_leds_notify_brightness_change_extern();
        assert(notifications == 1 && clevo_led_cdev.brightness == 2);
    } else if (!strcmp(name, "battery_empty_registration")) {
        clevo_flexicharger_init(); clevo_flexicharger_remove(); clevo_flexicharger_remove();
        assert(hook_registered == 1 && hook_unregistered == 1 && !charge_control_registered);
    } else if (!strncmp(name, "bind_", 5)) {
        if (!strcmp(name, "bind_failure")) {
            binding_status = -ENOMEM;
            assert(clevo_keyboard_add_interface(&wmi) == -ENOMEM);
            assert(!active_clevo_interface && !clevo_interfaces.wmi && !wmi.event_callb);
        } else {
            assert(!clevo_keyboard_add_interface(&wmi));
            assert(active_clevo_interface == &wmi && wmi.event_callb == clevo_keyboard_event_callb);
            if (!strcmp(name, "bind_duplicate") || !strcmp(name, "bind_wrong_remove")) {
                struct clevo_interface_t other = wmi;
                if (!strcmp(name, "bind_duplicate")) assert(clevo_keyboard_add_interface(&other) == -EBUSY);
                else assert(!clevo_keyboard_remove_interface(&other));
                assert(clevo_interfaces.wmi == &wmi && !removals);
            } else if (!strcmp(name, "bind_priority")) {
                assert(!clevo_keyboard_add_interface(&acpi) && active_clevo_interface == &acpi);
                assert(!clevo_keyboard_remove_interface(&wmi) && active_clevo_interface == &acpi && !removals);
                assert(!wmi.event_callb);
            } else {
                assert(!strcmp(name, "bind_success"));
                assert(!clevo_keyboard_remove_interface(&wmi));
                assert(!wmi.event_callb && !active_clevo_interface && removals == 1);
            }
        }
        assert(!clevo_keyboard_interface_modification_lock.held);
    } else if (!strcmp(name, "combo_no_input")) {
        clevo_send_cc_combo(); assert(!key_events);
        static struct input_dev input; clevo_keyboard_driver.input_device = &input;
        clevo_send_cc_combo(); assert(key_events == 6);
    } else return 2;
    assert(allocations == frees);
    return 0;
}
