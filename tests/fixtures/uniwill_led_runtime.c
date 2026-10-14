/* SPDX-License-Identifier: GPL-2.0-only */
/* Actual driver callbacks/initializers + official LED core excerpts.
 * Kernel types, bitops, queue/timer scheduling and EC transport are test doubles.
 */
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

typedef uint8_t u8;
enum led_brightness { LED_OFF = 0, LED_FULL = 255 };
#define ENOTSUPP 524
#define __always_unused __attribute__((unused))
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define LED_FUNCTION_KBD_BACKLIGHT "kbd_backlight"
#define LED_COLOR_ID_RED 1
#define LED_COLOR_ID_GREEN 2
#define LED_COLOR_ID_BLUE 3
#define LED_SUSPENDED (1UL << 0)
#define LED_UNREGISTERING (1UL << 1)
#define LED_CORE_SUSPENDRESUME (1UL << 16)
#define LED_HW_PLUGGABLE (1UL << 19)
#define LED_BRIGHT_HW_CHANGED (1UL << 21)
#define BIT(bit) (1UL << (bit))
#define LED_BLINK_SW 0
#define LED_BLINK_ONESHOT 1
#define LED_BLINK_ONESHOT_STOP 2
#define LED_BLINK_INVERT 3
#define LED_BLINK_BRIGHTNESS_CHANGE 4
#define LED_BLINK_DISABLE 5
#define LED_SET_BRIGHTNESS_OFF 6
#define LED_SET_BRIGHTNESS 7
#define LED_SET_BLINK 8
#define container_of(pointer, type, member) ((type *)((char *)(pointer) - offsetof(type, member)))
#define timer_container_of(variable, pointer, member) container_of(pointer, struct led_classdev, member)
#define min(a, b) ((a) < (b) ? (a) : (b))
#define pr_debug(...) ((void)0)
#define pr_err(...) (++driver_errors)
#define smp_mb__before_atomic() ((void)0)
struct device { void *driver_data; struct device *parent; };
struct platform_device { struct device dev; };
struct dmi_match { int slot; const char *substr; };
struct dmi_system_id { struct dmi_match matches[4]; };
#define DMI_SYS_VENDOR 1
#define DMI_PRODUCT_SKU 2
#define DMI_BOARD_NAME 3
#define DMI_MATCH(slot_value, string_value) { .slot = slot_value, .substr = string_value }
struct work_struct { bool pending; };
struct timer_list { bool pending; };
struct led_classdev {
    const char *name;
    unsigned int max_brightness, brightness;
    unsigned long flags, work_flags;
    void (*brightness_set)(struct led_classdev *, enum led_brightness);
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
    struct work_struct set_brightness_work;
    struct timer_list blink_timer;
    void *wq, *dev;
    void (*flash_resume)(struct led_classdev *);
    unsigned int delayed_set_value;
    unsigned long delayed_delay_on, delayed_delay_off;
    unsigned long blink_delay_on, blink_delay_off;
    unsigned long blink_brightness, new_blink_brightness;
};
struct mc_subled { unsigned int color_index, intensity, max_intensity, channel; };
struct led_classdev_mc {
    struct led_classdev led_cdev;
    unsigned int num_colors;
    struct mc_subled *subled_info;
};
#define lcdev_to_mccdev(pointer) container_of(pointer, struct led_classdev_mc, led_cdev)
static bool atomic_context;
static int ec_calls, fail_call, failure_errno, core_errors, core_last_errno;
static unsigned int queues, timer_updates, sleep_count;
static unsigned long initial_white_flags, initial_rgb_flags;
static int driver_errors, registration_errno, persistent_read_errno;
static unsigned int persistent_read_address;
static bool model_five_levels, model_force_no;
static struct led_classdev *registered_led;
static struct device child_device;
static struct platform_device parent_device;
static bool parent_disable_during_status_read;
static int model_parent_suspend_status, model_parent_resume_status;
static unsigned int parent_suspend_calls, parent_resume_calls;
static int parent_backlight(u8 enable);
static void flush_work(struct work_struct *work);
static int led_classdev_register(struct device *, struct led_classdev *);
static int devm_led_classdev_multicolor_register(struct device *, struct led_classdev_mc *);
static void led_classdev_unregister(struct led_classdev *);
static void devm_led_classdev_multicolor_unregister(struct device *, struct led_classdev_mc *);
static int dmi_check_system(const struct dmi_system_id *);
static void *dev_get_drvdata(struct device *dev) { return dev->driver_data; }
static void msleep(unsigned int value) { (void)value; assert(!atomic_context); ++sleep_count; }
static unsigned long jiffies;
static u8 ec_ram[65536];
struct transaction { bool write; unsigned int address, value; };
static struct transaction transactions[128];
void led_set_brightness_nosleep(struct led_classdev *, unsigned int);
static struct led_classdev *injected_led;
static unsigned int injected_brightness;
static int inject_call;
static void record(bool write, unsigned int address, unsigned int value)
{
    /* Real EC implementations acquire mutexes/allocate GFP_KERNEL or msleep. */
    assert(!atomic_context && "sleeping EC operation reached atomic LED dispatch");
    assert(ec_calls < 128);
    transactions[ec_calls++] = (struct transaction){ write, address, value };
    if (ec_calls == inject_call) {
        /* Simulate a newer atomic request while old EC work can sleep. */
        atomic_context = true;
        led_set_brightness_nosleep(injected_led, injected_brightness);
        atomic_context = false;
    }
}
static int uniwill_read_ec_ram(unsigned int address, u8 *value)
{
    record(false, address, 0);
    if (ec_calls == fail_call)
        return failure_errno;
    if (persistent_read_errno && address == persistent_read_address)
        return persistent_read_errno;
    *value = ec_ram[address];
    if (parent_disable_during_status_read && address == 0x078c) {
        /* An old RMW has captured enabled bits; model parent suspend now. */
        parent_disable_during_status_read = false;
        assert(parent_backlight(0) == 0);
    }
    return 0;
}
static int uniwill_write_ec_ram(unsigned int address, u8 value)
{
    record(true, address, value);
    if (ec_calls == fail_call)
        return failure_errno;
    ec_ram[address] = value;
    return 0;
}
static void set_bit(unsigned int bit, unsigned long *bits) { *bits |= 1UL << bit; }
static void clear_bit(unsigned int bit, unsigned long *bits) { *bits &= ~(1UL << bit); }
static bool test_bit(unsigned int bit, const unsigned long *bits) { return !!(*bits & (1UL << bit)); }
static bool test_and_clear_bit(unsigned int bit, unsigned long *bits)
{
    bool old = test_bit(bit, bits);
    clear_bit(bit, bits);
    return old;
}
static bool queue_work(void *queue, struct work_struct *work)
{
    (void)queue;
    bool was_pending = work->pending;
    work->pending = true;
    ++queues;
    return !was_pending;
}
static void dev_err(void *dev, const char *format, int error)
{
    (void)dev; (void)format;
    ++core_errors;
    core_last_errno = error;
}
static unsigned int led_get_brightness(struct led_classdev *led) { return led->brightness; }
static void mod_timer(struct timer_list *timer, unsigned long expires)
{
    (void)expires;
    timer->pending = true;
    ++timer_updates;
}
static unsigned long msecs_to_jiffies(unsigned long value) { return value; }
static void timer_delete_sync(struct timer_list *timer) { timer->pending = false; }
static void led_blink_set(struct led_classdev *led, unsigned long *on, unsigned long *off)
{
    (void)led; (void)on; (void)off;
    assert(false && "hardware blink path is outside this harness");
}
void led_set_brightness_nosleep(struct led_classdev *, unsigned int);
void led_stop_software_blink(struct led_classdev *);

/* @DRIVER_CONSTANTS@ */
/* @DRIVER_SOURCE@ */
/* @PARENT_BACKLIGHT_HELPER@ */
/* @ACTUAL_LED_CORE@ */
/* @ACTUAL_LED_PM@ */
/* @DRIVER_LIFECYCLE@ */

static int parent_backlight(u8 enable)
{
    /* @INVOKE_PARENT_BACKLIGHT@ */
}
static int dmi_check_system(const struct dmi_system_id *table)
{
    if (table == force_no_ec_led_control)
        return model_force_no;
    assert(table == kbl_type_fixed_color_5_levels);
    return model_five_levels;
}
static int led_classdev_register(struct device *parent, struct led_classdev *led)
{
    assert(!registered_led);
    if (registration_errno)
        return registration_errno;
    registered_led = led;
    child_device = (struct device){ .driver_data = led, .parent = parent };
    led->dev = &child_device;
    return 0;
}
static int devm_led_classdev_multicolor_register(struct device *parent, struct led_classdev_mc *led)
{
    return led_classdev_register(parent, &led->led_cdev);
}
static void led_classdev_unregister(struct led_classdev *led)
{
    /* Mock the relevant core unregister services and retained lifecycle flag. */
    assert(registered_led == led);
    led->flags |= LED_UNREGISTERING;
    led_stop_software_blink(led);
    led_set_brightness_nosleep(led, LED_OFF);
    flush_work(&led->set_brightness_work);
    registered_led = NULL;
}
static void devm_led_classdev_multicolor_unregister(struct device *parent, struct led_classdev_mc *led)
{
    assert(parent == &parent_device.dev);
    led_classdev_unregister(&led->led_cdev);
}

static void reset(void)
{
    atomic_context = false;
    ec_calls = fail_call = core_errors = core_last_errno = 0;
    failure_errno = -EIO;
    queues = timer_updates = sleep_count = 0;
    driver_errors = registration_errno = persistent_read_errno = 0;
    persistent_read_address = 0;
    model_parent_suspend_status = model_parent_resume_status = 0;
    parent_suspend_calls = parent_resume_calls = 0;
    model_five_levels = model_force_no = parent_disable_during_status_read = false;
    registered_led = NULL;
    uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_NONE;
    uniwill_barebone_id = 0;
    uniwill_kbl_brightness_ec_controlled = uw_leds_initialized = false;
    injected_led = NULL;
    injected_brightness = inject_call = 0;
    memset(ec_ram, 0, sizeof(ec_ram));
    memset(transactions, 0, sizeof(transactions));
    struct led_classdev *devices[] = { &uniwill_led_cdev, &uniwill_mcled_cdev.led_cdev };
    for (unsigned int i = 0; i < 2; ++i) {
        struct led_classdev *led = devices[i];
        led->brightness = led->work_flags = 0;
        led->flags = i ? initial_rgb_flags : initial_white_flags;
        led->blink_delay_on = led->blink_delay_off = led->blink_brightness = 0;
        led->new_blink_brightness = led->delayed_set_value = 0;
        led->delayed_delay_on = led->delayed_delay_off = 0;
        led->set_brightness_work.pending = led->blink_timer.pending = false;
    }
    /* Five-level white models raise this core limit at registration. */
    uniwill_led_cdev.max_brightness = UNIWILL_KBD_BRIGHTNESS_MAX_WHITE_5;
    for (unsigned int i = 0; i < 3; ++i)
        uw_mcled_cdev_subleds[i].intensity = 255;
}
static int invoke(struct led_classdev *led, unsigned int brightness)
{
    /* The real core owns the request cache. Adapt the old callback API only
     * at the harness entry point, without rewriting any driver function. */
    if (led->brightness_set_blocking)
        return led_set_brightness_sync(led, brightness);
    assert(led->brightness_set);
    led_set_brightness_nosleep(led, brightness);
    return 0;
}
static void run_work(struct led_classdev *led)
{
    assert(!atomic_context && led->set_brightness_work.pending);
    led->set_brightness_work.pending = false;
    set_brightness_delayed(&led->set_brightness_work);
}
static void flush_work(struct work_struct *work)
{
    struct led_classdev *led = container_of(work, struct led_classdev, set_brightness_work);
    assert(!atomic_context);
    while (work->pending)
        run_work(led);
}
static void expected(unsigned int index, bool write, unsigned int address, unsigned int value)
{
    assert((int)index < ec_calls);
    assert(transactions[index].write == write);
    assert(transactions[index].address == address);
    if (write)
        assert(transactions[index].value == value);
}
static unsigned int status_value(unsigned int byte, unsigned int brightness)
{
    return (byte & 0x0f) | (brightness << 5) | UW_EC_REG_KBD_BL_STATUS_SUBCMD_RESET;
}
static void dispatch(bool rgb, bool timer)
{
    reset();
    struct led_classdev *led = rgb ? &uniwill_mcled_cdev.led_cdev : &uniwill_led_cdev;
    atomic_context = true;
    if (timer) {
        led->blink_delay_on = 100;
        led->blink_delay_off = 100;
        led->blink_brightness = 2;
        led_timer_function(&led->blink_timer);
        assert(timer_updates == 1 && led->blink_timer.pending);
    } else {
        led_set_brightness_nosleep(led, 2);
    }
    assert(ec_calls == 0 && queues == 1 && led->set_brightness_work.pending);
    atomic_context = false;
    run_work(led);
    assert(ec_calls == (rgb ? 7 : 4));
    assert(led->brightness == 2 && core_errors == 0);
    if (timer) {
        ec_calls = 0;
        atomic_context = true;
        led_timer_function(&led->blink_timer);
        assert(ec_calls == 0 && queues == 2 && timer_updates == 2);
        atomic_context = false;
        run_work(led);
        assert(ec_calls == (rgb ? 7 : 3) && led->brightness == 0);
    }
}
static void white_success(void)
{
    for (unsigned int byte = 0; byte < 256; ++byte) {
        for (unsigned int level = 0; level <= 4; ++level) {
            reset();
            /* Five-level models adjust this core limit during registration. */
            uniwill_led_cdev.max_brightness = 4;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = byte;
            assert(invoke(&uniwill_led_cdev, level) == 0);
            unsigned int nudge = !!level;
            assert(ec_calls == (int)(3 + nudge));
            expected(0, false, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 0);
            if (nudge)
                expected(1, true, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 1);
            expected(1 + nudge, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(2 + nudge, true, UW_EC_REG_KBD_BL_STATUS, status_value(byte, level));
            assert(uniwill_led_cdev.brightness == level);
        }
    }
}
static void white_no_nudge(void)
{
    for (unsigned int immediate = 1; immediate < 256; ++immediate) {
        for (unsigned int level = 0; level <= 4; ++level) {
            reset();
            ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] = immediate;
            assert(invoke(&uniwill_led_cdev, level) == 0);
            assert(ec_calls == 3);
            expected(0, false, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 0);
            expected(1, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(2, true, UW_EC_REG_KBD_BL_STATUS, status_value(0, level));
            assert(ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] == immediate);
        }
    }
    reset();
    assert(invoke(&uniwill_led_cdev, 0) == 0 && ec_calls == 3);
    assert(ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] == 0);
}
static void white_failures(void)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int error = 0; error < sizeof(errors) / sizeof(errors[0]); ++error) {
        for (int call = 1; call <= 4; ++call) {
            reset();
            uniwill_led_cdev.brightness = 1;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = 0x8a;
            fail_call = call;
            failure_errno = errors[error];
            assert(invoke(&uniwill_led_cdev, 2) == errors[error]);
            assert(ec_calls == call);
            assert(uniwill_led_cdev.brightness == 2);
            assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == 0x8a);
            assert(ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] == (call > 2));
        }
    }
}
static void rgb_success(void)
{
    for (unsigned int byte = 0; byte < 256; ++byte) {
        for (unsigned int level = 0; level <= 4; ++level) {
            reset();
            ec_ram[UW_EC_REG_KBD_BL_RGB_MODE] = byte;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = byte;
            uw_mcled_cdev_subleds[0].intensity = 255;
            uw_mcled_cdev_subleds[1].intensity = 127;
            uw_mcled_cdev_subleds[2].intensity = 5;
            assert(invoke(&uniwill_mcled_cdev.led_cdev, level) == 0);
            assert(ec_calls == 7);
            expected(0, true, UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS, 50);
            expected(1, true, UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS, 24);
            expected(2, true, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS, 0);
            expected(3, false, UW_EC_REG_KBD_BL_RGB_MODE, 0);
            expected(4, true, UW_EC_REG_KBD_BL_RGB_MODE, byte | UW_EC_REG_KBD_BL_RGB_MODE_BIT_APPLY_COLOR);
            expected(5, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(6, true, UW_EC_REG_KBD_BL_STATUS, status_value(byte, level));
            assert(uniwill_mcled_cdev.led_cdev.brightness == level);
        }
    }
}
static void rgb_conversion(void)
{
    unsigned int addresses[] = { UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS,
                                 UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS,
                                 UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS };
    for (unsigned int channel = 0; channel < 3; ++channel) {
        for (unsigned int intensity = 0; intensity < 256; ++intensity) {
            reset();
            for (unsigned int i = 0; i < 3; ++i)
                uw_mcled_cdev_subleds[i].intensity = i == channel ? intensity : 1;
            assert(invoke(&uniwill_mcled_cdev.led_cdev, 4) == 0 && ec_calls == 7);
            expected(channel, true, addresses[channel], intensity * 50 / 255);
        }
    }
}
static void rgb_black(void)
{
    for (unsigned int byte = 0; byte < 256; ++byte) {
        for (unsigned int level = 0; level <= 4; ++level) {
            reset();
            for (unsigned int i = 0; i < 3; ++i)
                uw_mcled_cdev_subleds[i].intensity = 0;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = byte;
            assert(invoke(&uniwill_mcled_cdev.led_cdev, level) == 0 && ec_calls == 2);
            expected(0, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(1, true, UW_EC_REG_KBD_BL_STATUS, status_value(byte, 0));
            assert(uniwill_mcled_cdev.led_cdev.brightness == level);
        }
    }
}
static void rgb_failures(bool black)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int error = 0; error < sizeof(errors) / sizeof(errors[0]); ++error) {
        for (int call = 1; call <= (black ? 2 : 7); ++call) {
            reset();
            for (unsigned int i = 0; i < 3; ++i)
                uw_mcled_cdev_subleds[i].intensity = black ? 0 : 255;
            uniwill_mcled_cdev.led_cdev.brightness = 1;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = 0x8a;
            ec_ram[UW_EC_REG_KBD_BL_RGB_MODE] = 0x89;
            fail_call = call;
            failure_errno = errors[error];
            assert(invoke(&uniwill_mcled_cdev.led_cdev, 4) == errors[error]);
            assert(ec_calls == call);
            assert(uniwill_mcled_cdev.led_cdev.brightness == 4);
            assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == 0x8a);
            if (!black) {
                assert(ec_ram[UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS] == (call > 1 ? 50 : 0));
                assert(ec_ram[UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS] == (call > 2 ? 50 : 0));
                assert(ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS] == (call > 3 ? 50 : 0));
                assert(ec_ram[UW_EC_REG_KBD_BL_RGB_MODE] == (call > 5 ? 0xa9 : 0x89));
            }
        }
    }
}
static void work_error(void)
{
    for (unsigned int rgb = 0; rgb < 2; ++rgb) {
        reset();
        struct led_classdev *led = rgb ? &uniwill_mcled_cdev.led_cdev : &uniwill_led_cdev;
        fail_call = 1;
        failure_errno = -EREMOTEIO;
        atomic_context = true;
        led_set_brightness_nosleep(led, 2);
        assert(ec_calls == 0);
        atomic_context = false;
        run_work(led);
        assert(core_errors == 1 && core_last_errno == -EREMOTEIO && ec_calls == 1);
        /* Core records the request before EC runs; this is not confirmed hardware state. */
        assert(led->brightness == 2);
    }
}
static void sync_error(void)
{
    for (unsigned int rgb = 0; rgb < 2; ++rgb) {
        reset();
        struct led_classdev *led = rgb ? &uniwill_mcled_cdev.led_cdev : &uniwill_led_cdev;
        fail_call = 1;
        failure_errno = -ETIMEDOUT;
        assert(led_set_brightness_sync(led, 2) == -ETIMEDOUT);
        assert(ec_calls == 1 && queues == 0 && led->brightness == 2);
    }
}
static void off_on(void)
{
    for (unsigned int rgb = 0; rgb < 2; ++rgb) {
        reset();
        struct led_classdev *led = rgb ? &uniwill_mcled_cdev.led_cdev : &uniwill_led_cdev;
        ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] = 1;
        atomic_context = true;
        led_set_brightness_nosleep(led, 0);
        led_set_brightness_nosleep(led, 2);
        assert(ec_calls == 0 && queues == 2);
        atomic_context = false;
        run_work(led);
        unsigned int length = rgb ? 7 : 3;
        assert(ec_calls == (int)(length * 2));
        expected(length - 1, true, UW_EC_REG_KBD_BL_STATUS, status_value(0, 0));
        expected(2 * length - 1, true, UW_EC_REG_KBD_BL_STATUS, status_value(0, 2));
        assert(led->brightness == 2 && core_errors == 0);
    }
}
static void cache_interleave(bool rgb)
{
    reset();
    struct led_classdev *led = rgb ? &uniwill_mcled_cdev.led_cdev : &uniwill_led_cdev;
    atomic_context = true;
    led_set_brightness_nosleep(led, 1);
    atomic_context = false;
    assert(ec_calls == 0 && led->brightness == 1);
    injected_led = led;
    injected_brightness = 2;
    inject_call = 1;
    run_work(led);
    /* The earlier callback must not overwrite the newer core request cache. */
    assert(led->brightness == 2 && led->set_brightness_work.pending);
    assert(queues == 2 && core_errors == 0);
    inject_call = 0;
    ec_calls = 0;
    run_work(led);
    assert(led->brightness == 2 && core_errors == 0);
    assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == status_value(0, 2));
}
static struct led_classdev *init_model(unsigned int model)
{
    model_five_levels = model == 1;
    model_force_no = model == 3;
    ec_ram[UW_EC_REG_BAREBONE_ID] = model == 0 ? UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx : 1;
    ec_ram[UW_EC_REG_FEATURES_1] = model == 2 ? UW_EC_REG_FEATURES_1_BIT_1_ZONE_RGB_KB : 0;
    assert(uniwill_leds_init(&parent_device) == 0);
    if (model < 3)
        assert(registered_led);
    else
        assert(!registered_led);
    return registered_led;
}
static void clear_transactions(void)
{
    assert(!atomic_context);
    ec_calls = fail_call = 0;
    memset(transactions, 0, sizeof(transactions));
}
static void pm_initial_flags(void)
{
    assert(initial_white_flags & LED_CORE_SUSPENDRESUME);
    assert(initial_rgb_flags & LED_CORE_SUSPENDRESUME);
}
static void init_flags(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        assert(led->flags == (LED_CORE_SUSPENDRESUME | LED_BRIGHT_HW_CHANGED));
        assert(child_device.parent == &parent_device.dev);
        assert(uw_leds_initialized);
    }
}
static void pm_queued_rmw(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        clear_transactions();
        atomic_context = true;
        led_set_brightness_nosleep(led, 2);
        atomic_context = false;
        assert(led->set_brightness_work.pending && ec_calls == 0);
        assert(led_suspend(&child_device) == 0);
        if (led->set_brightness_work.pending) {
            /* A missing CORE flag leaves old work able to outlive parent PM. */
            parent_disable_during_status_read = true;
            run_work(led);
        } else {
            assert(parent_backlight(0) == 0);
        }
        assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] & BIT(1));
        assert(!led->set_brightness_work.pending);
        assert(led->flags & LED_SUSPENDED);
        assert(led->brightness == 2);
    }
}
static void pm_latest_request(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        assert(invoke(led, 2) == 0);
        clear_transactions();
        assert(led_suspend(&child_device) == 0);
        assert(led->brightness == 2);
        assert(!(ec_ram[UW_EC_REG_KBD_BL_STATUS] & 0xe0));
        assert(parent_backlight(0) == 0);
        clear_transactions();
        atomic_context = true;
        led_set_brightness_nosleep(led, 1);
        atomic_context = false;
        assert(led->brightness == 1 && ec_calls == 0 && !led->set_brightness_work.pending);
        assert(parent_backlight(1) == 0);
        assert(led_resume(&child_device) == 0);
        assert(!(led->flags & LED_SUSPENDED) && led->set_brightness_work.pending);
        run_work(led);
        assert(led->brightness == 1);
        assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == status_value(0, 1));
    }
}
/* Explicit hierarchy-order/failure model, not an extraction of full DPM. */
static int hierarchy_suspend(void)
{
    assert(led_suspend(&child_device) == 0);
    ++parent_suspend_calls;
    int result = model_parent_suspend_status;
    if (!result)
        result = parent_backlight(0);
    if (result)
        assert(led_resume(&child_device) == 0);
    return result;
}
static int hierarchy_resume(void)
{
    ++parent_resume_calls;
    int result = model_parent_resume_status;
    if (!result)
        result = parent_backlight(1);
    /* DPM completes a parent resume even on error and runs child resume. */
    assert(led_resume(&child_device) == 0);
    return result;
}
static void pm_suspend_abort(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        assert(invoke(led, 2) == 0);
        clear_transactions();
        model_parent_suspend_status = -EIO;
        assert(hierarchy_suspend() == -EIO && parent_suspend_calls == 1);
        assert(!(led->flags & LED_SUSPENDED) && led->set_brightness_work.pending);
        run_work(led);
        assert(!(led->flags & LED_SUSPENDED) && led->brightness == 2);
        assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == status_value(0, 2));
    }
}
static void pm_resume_error(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        assert(invoke(led, 2) == 0);
        clear_transactions();
        assert(hierarchy_suspend() == 0 && parent_suspend_calls == 1);
        model_parent_resume_status = -EIO;
        assert(hierarchy_resume() == -EIO && parent_resume_calls == 1);
        assert(!(led->flags & LED_SUSPENDED) && led->set_brightness_work.pending);
        run_work(led);
        assert(!(led->flags & LED_SUSPENDED) && led->brightness == 2);
        assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == status_value(BIT(1), 2));
    }
}
static void pm_ec_errors(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        for (unsigned int restore = 0; restore < 2; ++restore) {
            reset();
            struct led_classdev *led = init_model(model);
            assert(invoke(led, 2) == 0);
            clear_transactions();
            failure_errno = -EREMOTEIO;
            if (!restore)
                fail_call = 1;
            assert(led_suspend(&child_device) == 0);
            assert(led->flags & LED_SUSPENDED);
            assert(led->brightness == 2 && core_errors == (!restore));
            clear_transactions();
            if (restore)
                fail_call = 1;
            assert(led_resume(&child_device) == 0);
            run_work(led);
            assert(!(led->flags & LED_SUSPENDED));
            assert(led->brightness == 2 && core_errors == 1 && core_last_errno == -EREMOTEIO);
        }
    }
}
static void init_feature_bytes(void)
{
    for (unsigned int byte = 0; byte < 256; ++byte) {
        reset();
        ec_ram[UW_EC_REG_BAREBONE_ID] = UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx;
        ec_ram[UW_EC_REG_FEATURES_1] = byte;
        assert(uniwill_leds_init(&parent_device) == 0 && registered_led);
        assert(ec_ram[UW_EC_REG_FEATURES_1] == (byte | UW_EC_REG_FEATURES_1_BIT_FIXED_COLOR_5_ENABLE));
        assert(ec_calls == 3);
        expected(1, false, UW_EC_REG_FEATURES_1, 0);
        expected(2, true, UW_EC_REG_FEATURES_1, byte | UW_EC_REG_FEATURES_1_BIT_FIXED_COLOR_5_ENABLE);
    }
}
static void init_feature_failures(void)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int error = 0; error < sizeof(errors) / sizeof(errors[0]); ++error) {
        for (int call = 2; call <= 4; ++call) {
            reset();
            ec_ram[UW_EC_REG_BAREBONE_ID] = call == 4 ? 1 : UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx;
            ec_ram[UW_EC_REG_FEATURES_1] = 0x89;
            fail_call = call;
            failure_errno = errors[error];
            assert(uniwill_leds_init(&parent_device) == errors[error]);
            assert(ec_calls == call && driver_errors == 1);
            assert(!registered_led && !uw_leds_initialized);
            assert(uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_NONE);
            assert(ec_ram[UW_EC_REG_FEATURES_1] == (call == 4 ? 0xa9 : 0x89));
            clear_transactions();
            uniwill_leds_restore_state_extern();
            assert(ec_calls == 0);
        }
    }
}
static void init_barebone_errors(void)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int error = 0; error < sizeof(errors) / sizeof(errors[0]); ++error) {
        reset();
        persistent_read_address = UW_EC_REG_BAREBONE_ID;
        persistent_read_errno = errors[error];
        assert(uniwill_leds_init(&parent_device) == errors[error]);
        assert(ec_calls == 3 && sleep_count == 3 && !registered_led && !uw_leds_initialized);
        assert(uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_NONE);
    }
}
static void init_zero_barebone(void)
{
    reset();
    assert(uniwill_leds_init(&parent_device) == -EIO);
    assert(ec_calls == 3 && sleep_count == 3 && !registered_led && !uw_leds_initialized);
}
static void init_rebind(void)
{
    reset();
    struct led_classdev *led = init_model(1);
    assert(led->max_brightness == 4);
    assert(invoke(led, 4) == 0 && led->brightness == 4);
    assert(led_suspend(&child_device) == 0 && (led->flags & LED_SUSPENDED));
    assert(uniwill_leds_remove(&parent_device) == 0 && !registered_led);
    /* Actual child suspend then driver remove leaves both lifecycle flags. */
    assert((led->flags & (LED_UNREGISTERING | LED_SUSPENDED)) == (LED_UNREGISTERING | LED_SUSPENDED));
    assert(led->brightness == 0);
    clear_transactions();
    led = init_model(0);
    assert(led->flags == (LED_CORE_SUSPENDRESUME | LED_BRIGHT_HW_CHANGED));
    assert(led->max_brightness == 2 && led->brightness == 0);
    assert(led_suspend(&child_device) == 0 && led_resume(&child_device) == 0);
    flush_work(&led->set_brightness_work);
    assert(led->brightness <= led->max_brightness);
    assert(uniwill_leds_remove(&parent_device) == 0 && !registered_led);
    clear_transactions();
    assert(init_model(3) == NULL);
    assert(uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_NONE);
    assert(!uniwill_kbl_brightness_ec_controlled);
    assert(uniwill_leds_remove(&parent_device) == 0);
    clear_transactions();
    led = init_model(2);
    assert(led == &uniwill_mcled_cdev.led_cdev);
    assert(led->flags == (LED_CORE_SUSPENDRESUME | LED_BRIGHT_HW_CHANGED));
}
static void init_registration_error(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        model_five_levels = model == 1;
        ec_ram[UW_EC_REG_BAREBONE_ID] = model == 0 ? UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx : 1;
        ec_ram[UW_EC_REG_FEATURES_1] = model == 2 ? UW_EC_REG_FEATURES_1_BIT_1_ZONE_RGB_KB : 0;
        registration_errno = -ENOMEM;
        assert(uniwill_leds_init(&parent_device) == -ENOMEM);
        assert(!registered_led && !uw_leds_initialized);
        assert(uniwill_kb_backlight_type == (model == 0 ? UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR :
               model == 1 ? UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS : UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB));
        clear_transactions();
        /* Capability remains known, but failed registration has no LED state. */
        uniwill_leds_restore_state_extern();
        assert(ec_calls == 0 && !registered_led && !uw_leds_initialized);
    }
}
static void restore_success(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        struct led_classdev *led = init_model(model);
        assert(invoke(led, 2) == 0);
        ec_ram[UW_EC_REG_KBD_BL_STATUS] = 0x8a;
        ec_ram[UW_EC_REG_KBD_BL_RGB_MODE] = 0x89;
        ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] = 0;
        clear_transactions();
        uniwill_leds_restore_state_extern();
        assert(led->brightness == 2);
        if (model == 2) {
            assert(ec_calls == 7);
            expected(0, true, UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS, 50);
            expected(1, true, UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS, 50);
            expected(2, true, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS, 50);
            expected(3, false, UW_EC_REG_KBD_BL_RGB_MODE, 0);
            expected(4, true, UW_EC_REG_KBD_BL_RGB_MODE, 0xa9);
            expected(5, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(6, true, UW_EC_REG_KBD_BL_STATUS, status_value(0x8a, 2));
        } else {
            assert(ec_calls == 4);
            expected(0, false, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 0);
            expected(1, true, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 1);
            expected(2, false, UW_EC_REG_KBD_BL_STATUS, 0);
            expected(3, true, UW_EC_REG_KBD_BL_STATUS, status_value(0x8a, 2));
        }
    }
}
static void restore_after_remove(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        reset();
        init_model(model);
        assert(uniwill_leds_remove(&parent_device) == 0 && !registered_led);
        clear_transactions();
        uniwill_leds_restore_state_extern();
        assert(ec_calls == 0 && !uw_leds_initialized);
    }
}
static void restore_errors(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        for (int call = 1; call <= (model == 2 ? 7 : 4); ++call) {
            reset();
            struct led_classdev *led = init_model(model);
            assert(invoke(led, 2) == 0);
            ec_ram[UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE] = 0;
            clear_transactions();
            fail_call = call;
            failure_errno = -ETIMEDOUT;
            uniwill_leds_restore_state_extern();
            assert(led->brightness == 2 && uw_leds_initialized);
            /* Existing RGB restore independently applies brightness after a color error. */
            assert(ec_calls == (model == 2 && call <= 5 ? call + 2 : call));
        }
    }
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    initial_white_flags = uniwill_led_cdev.flags;
    initial_rgb_flags = uniwill_mcled_cdev.led_cdev.flags;
    if (!strcmp(argv[1], "white_dispatch")) dispatch(false, false);
    else if (!strcmp(argv[1], "rgb_dispatch")) dispatch(true, false);
    else if (!strcmp(argv[1], "white_timer")) dispatch(false, true);
    else if (!strcmp(argv[1], "rgb_timer")) dispatch(true, true);
    else if (!strcmp(argv[1], "white_success")) white_success();
    else if (!strcmp(argv[1], "white_no_nudge")) white_no_nudge();
    else if (!strcmp(argv[1], "white_failures")) white_failures();
    else if (!strcmp(argv[1], "rgb_success")) rgb_success();
    else if (!strcmp(argv[1], "rgb_conversion")) rgb_conversion();
    else if (!strcmp(argv[1], "rgb_black")) rgb_black();
    else if (!strcmp(argv[1], "rgb_failures")) rgb_failures(false);
    else if (!strcmp(argv[1], "black_failures")) rgb_failures(true);
    else if (!strcmp(argv[1], "work_error")) work_error();
    else if (!strcmp(argv[1], "sync_error")) sync_error();
    else if (!strcmp(argv[1], "off_on")) off_on();
    else if (!strcmp(argv[1], "white_cache_interleave")) cache_interleave(false);
    else if (!strcmp(argv[1], "rgb_cache_interleave")) cache_interleave(true);
    else if (!strcmp(argv[1], "pm_initial_flags")) pm_initial_flags();
    else if (!strcmp(argv[1], "init_flags")) init_flags();
    else if (!strcmp(argv[1], "pm_queued_rmw")) pm_queued_rmw();
    else if (!strcmp(argv[1], "pm_latest_request")) pm_latest_request();
    else if (!strcmp(argv[1], "pm_suspend_abort")) pm_suspend_abort();
    else if (!strcmp(argv[1], "pm_resume_error")) pm_resume_error();
    else if (!strcmp(argv[1], "pm_ec_errors")) pm_ec_errors();
    else if (!strcmp(argv[1], "init_feature_bytes")) init_feature_bytes();
    else if (!strcmp(argv[1], "init_feature_failures")) init_feature_failures();
    else if (!strcmp(argv[1], "init_zero_barebone")) init_zero_barebone();
    else if (!strcmp(argv[1], "init_barebone_errors")) init_barebone_errors();
    else if (!strcmp(argv[1], "init_rebind")) init_rebind();
    else if (!strcmp(argv[1], "init_registration_error")) init_registration_error();
    else if (!strcmp(argv[1], "restore_success")) restore_success();
    else if (!strcmp(argv[1], "restore_after_remove")) restore_after_remove();
    else if (!strcmp(argv[1], "restore_errors")) restore_errors();
    else return 2;
    return 0;
}
