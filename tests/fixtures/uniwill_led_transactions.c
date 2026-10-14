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
#include <pthread.h>
#include <stdatomic.h>
#include <time.h>
typedef uint16_t u16;
#define READ_ONCE(x) __atomic_load_n(&(x), __ATOMIC_ACQUIRE)
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
struct work_struct { _Atomic bool pending; };
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
static _Thread_local bool atomic_context;
static int ec_calls, fail_call, failure_errno, core_errors, core_last_errno;
static unsigned int queues, timer_updates, sleep_count;
static unsigned long initial_white_flags, initial_rgb_flags;
static int driver_errors, registration_errno, persistent_read_errno;
static unsigned int persistent_read_address;
static bool model_five_levels, model_force_no;
static struct led_classdev *registered_led;
static struct device child_device;
static struct platform_device parent_device;
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
static unsigned int hardware_events, hardware_last_brightness;
static void led_classdev_notify_brightness_hw_changed(struct led_classdev *led, unsigned int value)
{ (void)led; ++hardware_events; hardware_last_brightness = value; }
void led_set_brightness_nosleep(struct led_classdev *, unsigned int);
/* Deterministic scheduling happens after a real pthread mutex unlock. */
struct mutex { pthread_mutex_t value; const char *name; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER, #name }
static pthread_mutex_t schedule_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t schedule_changed = PTHREAD_COND_INITIALIZER;
static pthread_t first_thread;
static bool schedule_active, first_paused, release_first;
static bool second_blocked, second_done, second_started;
static bool pause_any_ec_unlock, unregister_pause, unregister_started, allow_unregister;
static int pause_call;
static _Thread_local int thread_ec_calls;
static void signal_schedule(void) { assert(!pthread_cond_broadcast(&schedule_changed)); }
static void mutex_lock(struct mutex *mutex)
{
    int status = pthread_mutex_trylock(&mutex->value);
    if (status == EBUSY) {
        assert(!pthread_mutex_lock(&schedule_lock));
        if (schedule_active && !pthread_equal(pthread_self(), first_thread) &&
            strstr(mutex->name, "uniwill_led_lock")) {
            second_blocked = true;
            signal_schedule();
        }
        assert(!pthread_mutex_unlock(&schedule_lock));
        status = pthread_mutex_lock(&mutex->value);
    }
    assert(!status);
}
static void mutex_unlock(struct mutex *mutex)
{
    assert(!pthread_mutex_unlock(&mutex->value));
    assert(!pthread_mutex_lock(&schedule_lock));
    if (schedule_active && pthread_equal(pthread_self(), first_thread) &&
        strstr(mutex->name, "uniwill_ec_transaction_lock") && !first_paused &&
        (pause_any_ec_unlock || thread_ec_calls == pause_call)) {
        first_paused = true;
        signal_schedule();
        while (!release_first)
            assert(!pthread_cond_wait(&schedule_changed, &schedule_lock));
    }
    assert(!pthread_mutex_unlock(&schedule_lock));
}
static void record(bool write, unsigned int address, unsigned int value)
{
    assert(!atomic_context && "sleeping EC operation reached atomic LED dispatch");
    assert(ec_calls < 128);
    transactions[ec_calls++] = (struct transaction){ write, address, value };
    ++thread_ec_calls;
}
static int uniwill_read_ec_ram_unlocked(u16 address, u8 *value)
{
    record(false, address, 0);
    if (ec_calls == fail_call) {
        assert(failure_errno < 0);
        return failure_errno;
    }
    if (persistent_read_errno && address == persistent_read_address) {
        assert(persistent_read_errno < 0);
        return persistent_read_errno;
    }
    *value = ec_ram[address];
    return 0;
}
static int uniwill_write_ec_ram_unlocked(u16 address, u8 value)
{
    record(true, address, value);
    if (ec_calls == fail_call)
        return failure_errno;
    ec_ram[address] = value;
    return 0;
}
static void set_bit(unsigned int bit, unsigned long *bits) { __atomic_fetch_or(bits, 1UL << bit, __ATOMIC_SEQ_CST); }
static void clear_bit(unsigned int bit, unsigned long *bits) { __atomic_fetch_and(bits, ~(1UL << bit), __ATOMIC_SEQ_CST); }
static bool test_bit(unsigned int bit, const unsigned long *bits) { return !!(__atomic_load_n(bits, __ATOMIC_SEQ_CST) & (1UL << bit)); }
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
/* @ACTUAL_EC_API@ */
/* @DRIVER_DECLARATIONS@ */
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
    /* Registration services may invoke a callback. This deliberately checks
     * that driver init does not hold its LED mutex across that boundary. */
    assert(!uw_leds_initialized);
#if HAS_LED_LOCK
    assert(!pthread_mutex_trylock(&uniwill_led_lock.value));
    assert(!pthread_mutex_unlock(&uniwill_led_lock.value));
#endif
    assert(led_set_brightness_sync(led, led->brightness) == 0);
    return 0;
}
static int devm_led_classdev_multicolor_register(struct device *parent, struct led_classdev_mc *led)
{
    return led_classdev_register(parent, &led->led_cdev);
}
static void led_classdev_unregister(struct led_classdev *led)
{
    /* Services only: the OFF queueing and callback body are actual LED core. */
    assert(!pthread_mutex_lock(&schedule_lock));
    if (unregister_pause) {
        unregister_started = true;
        signal_schedule();
        while (!allow_unregister)
            assert(!pthread_cond_wait(&schedule_changed, &schedule_lock));
    }
    assert(!pthread_mutex_unlock(&schedule_lock));
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
    hardware_events = hardware_last_brightness = 0;
    schedule_active = first_paused = release_first = false;
    second_blocked = second_done = second_started = false;
    pause_any_ec_unlock = unregister_pause = unregister_started = allow_unregister = false;
    pause_call = thread_ec_calls = 0;
    ec_calls = fail_call = core_errors = core_last_errno = 0;
    failure_errno = -EIO;
    queues = timer_updates = sleep_count = 0;
    driver_errors = registration_errno = persistent_read_errno = 0;
    persistent_read_address = 0;
    model_five_levels = model_force_no = false;
    registered_led = NULL;
    uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_NONE;
    uniwill_barebone_id = 0;
    uniwill_kbl_brightness_ec_controlled = uw_leds_initialized = false;
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

static struct led_classdev *init_model(unsigned int model)
{
    reset();
    ec_ram[UW_EC_REG_BAREBONE_ID] = model == 0 ? UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx : 0x31;
    model_five_levels = model == 1;
    ec_ram[UW_EC_REG_FEATURES_1] = model == 2 ? UW_EC_REG_FEATURES_1_BIT_1_ZONE_RGB_KB : 0;
    assert(uniwill_leds_init(&parent_device) == 0);
    assert(uw_leds_initialized && registered_led);
    assert(uniwill_leds_get_backlight_type_extern() == (model == 0 ? UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR :
           model == 1 ? UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS : UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB));
    ec_calls = thread_ec_calls = 0;
    memset(transactions, 0, sizeof(transactions));
    return registered_led;
}
static void notify_read_errors(void)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int model = 0; model < 3; ++model) {
        for (unsigned int e = 0; e < sizeof(errors) / sizeof(errors[0]); ++e) {
            for (unsigned int cache = 0; cache <= 2; ++cache) {
                struct led_classdev *led = init_model(model);
                led->brightness = cache;
                fail_call = 1;
                failure_errno = errors[e];
                assert(uniwill_leds_notify_brightness_change_extern());
                assert(ec_calls == 1 && !transactions[0].write);
                assert(led->brightness == cache && hardware_events == 0);
                fail_call = 0;
                ec_ram[UW_EC_REG_KBD_BL_STATUS] = 1 << 5;
                assert(uniwill_leds_notify_brightness_change_extern());
                assert(hardware_events == 1 && led->brightness == (model == 2 && cache == 1 ? 2 : 1));
            }
        }
    }
}
static void notify_compensation_errors(void)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int e = 0; e < sizeof(errors) / sizeof(errors[0]); ++e) {
        for (int fail = 2; fail <= 3; ++fail) {
            for (unsigned int cache = 0; cache <= 4; ++cache) {
                struct led_classdev *led = init_model(2);
                led->brightness = cache;
                ec_ram[UW_EC_REG_KBD_BL_STATUS] = (cache << 5) | 0x0b;
                fail_call = fail;
                failure_errno = errors[e];
                assert(uniwill_leds_notify_brightness_change_extern());
                assert(ec_calls == fail && led->brightness == cache && !hardware_events);
                assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == ((cache << 5) | 0x0b));
                fail_call = 0;
                assert(uniwill_leds_notify_brightness_change_extern());
                assert(led->brightness == (cache + 1) % 5 && hardware_events == 1);
            }
        }
    }
}
static void notify_success(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        for (unsigned int value = 0; value <= 4; ++value) {
            struct led_classdev *led = init_model(model);
            led->brightness = value;
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = (value << 5) | 0x0b;
            assert(uniwill_leds_notify_brightness_change_extern());
            unsigned int result = model == 2 ? (value + 1) % 5 : value;
            assert(led->brightness == result && hardware_events == 1 && hardware_last_brightness == result);
            assert(ec_calls == (model == 2 ? 3 : 1));
            if (model == 2)
                assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == status_value(0x0b, result));
        }
    }
}
static void notify_gate(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        struct led_classdev *led = init_model(model);
        led->brightness = 2;
        uw_leds_initialized = false;
        assert(!uniwill_leds_notify_brightness_change_extern());
        uniwill_leds_restore_state_extern();
        assert(!ec_calls && !hardware_events && led->brightness == 2);
        uw_leds_initialized = true;
        uniwill_kbl_brightness_ec_controlled = false;
        assert(!uniwill_leds_notify_brightness_change_extern());
        assert(!ec_calls && !hardware_events && led->brightness == 2);
        uniwill_kbl_brightness_ec_controlled = true;
        uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_NONE;
        assert(!uniwill_leds_notify_brightness_change_extern());
        assert(!ec_calls && !hardware_events && led->brightness == 2);
    }
}
static void notify_suspended(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        struct led_classdev *led = init_model(model);
        led->brightness = 2;
        led_classdev_suspend(led);
        assert(led->flags & LED_SUSPENDED);
        assert(led->brightness == 2 && (ec_ram[UW_EC_REG_KBD_BL_STATUS] >> 5) == 0);
        int before = ec_calls;
        assert(uniwill_leds_notify_brightness_change_extern());
        assert(ec_calls == before && led->brightness == 2 && !hardware_events);
        led_classdev_resume(led);
        flush_work(&led->set_brightness_work);
        assert(!(led->flags & LED_SUSPENDED));
        assert(led->brightness == 2 && (ec_ram[UW_EC_REG_KBD_BL_STATUS] >> 5) == 2);
    }
}
struct operation { int kind, result; struct led_classdev *led; unsigned int value; };
enum { OP_CALLBACK, OP_RESTORE, OP_BRIGHTNESS, OP_PARENT, OP_NOTIFY, OP_SUSPEND, OP_REMOVE };
static void *perform(void *pointer)
{
    struct operation *operation = pointer;
    thread_ec_calls = 0;
    assert(!pthread_mutex_lock(&schedule_lock));
    if (schedule_active && !pthread_equal(pthread_self(), first_thread)) {
        second_started = true;
        signal_schedule();
    }
    assert(!pthread_mutex_unlock(&schedule_lock));
    switch (operation->kind) {
    case OP_CALLBACK: operation->result = invoke(operation->led, operation->value); break;
    case OP_RESTORE: uniwill_leds_restore_state_extern(); operation->result = 0; break;
    case OP_BRIGHTNESS: operation->result = uniwill_write_kbd_bl_brightness(operation->value); break;
    case OP_PARENT: operation->result = parent_backlight(operation->value); break;
    case OP_NOTIFY: operation->result = uniwill_leds_notify_brightness_change_extern(); break;
    case OP_SUSPEND: led_classdev_suspend(operation->led); operation->result = 0; break;
    case OP_REMOVE: operation->result = uniwill_leds_remove(&parent_device); break;
    default: assert(false);
    }
    assert(!pthread_mutex_lock(&schedule_lock));
    if (schedule_active && !pthread_equal(pthread_self(), first_thread)) {
        second_done = true;
        signal_schedule();
    }
    assert(!pthread_mutex_unlock(&schedule_lock));
    return NULL;
}
static void *first_perform(void *pointer)
{
    assert(!pthread_mutex_lock(&schedule_lock));
    first_thread = pthread_self();
    assert(!pthread_mutex_unlock(&schedule_lock));
    return perform(pointer);
}
static void wait_flag(const bool *flag)
{
    struct timespec limit;
    assert(!clock_gettime(CLOCK_REALTIME, &limit));
    limit.tv_sec += 5;
    assert(!pthread_mutex_lock(&schedule_lock));
    while (!*flag)
        assert(!pthread_cond_timedwait(&schedule_changed, &schedule_lock, &limit));
    assert(!pthread_mutex_unlock(&schedule_lock));
}
static void release_paused(void)
{
    assert(!pthread_mutex_lock(&schedule_lock));
    release_first = true;
    signal_schedule();
    assert(!pthread_mutex_unlock(&schedule_lock));
}
static bool wait_second_boundary(void)
{
    struct timespec limit;
    assert(!clock_gettime(CLOCK_REALTIME, &limit));
    limit.tv_sec += 5;
    assert(!pthread_mutex_lock(&schedule_lock));
    while (!second_blocked && !second_done)
        assert(!pthread_cond_timedwait(&schedule_changed, &schedule_lock, &limit));
    bool blocked = second_blocked;
    assert(!pthread_mutex_unlock(&schedule_lock));
    return blocked;
}
static void status_interleave(bool parent_first)
{
    for (unsigned int byte = 0; byte < 256; ++byte) {
        for (unsigned int enable = 0; enable < 2; ++enable) {
            reset();
            ec_ram[UW_EC_REG_KBD_BL_STATUS] = byte;
            struct operation brightness = { .kind = OP_BRIGHTNESS, .value = 3 };
            struct operation parent = { .kind = OP_PARENT, .value = enable };
            struct operation *first = parent_first ? &parent : &brightness;
            struct operation *second = parent_first ? &brightness : &parent;
            pthread_t a, b;
            schedule_active = pause_any_ec_unlock = true;
            assert(!pthread_create(&a, NULL, first_perform, first));
            wait_flag(&first_paused);
            assert(!pthread_create(&b, NULL, perform, second));
            wait_flag(&second_done);
            assert(!pthread_join(b, NULL));
            release_paused();
            assert(!pthread_join(a, NULL));
            assert(first->result == 0 && second->result == 0);
            unsigned int expected_byte = (status_value(byte, 3) & ~2U) | (!enable << 1);
            assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == expected_byte);
            assert(ec_calls == 4);
            /* Atomic API proves each read/write pair is contiguous too. */
            assert(!transactions[0].write && transactions[1].write &&
                   !transactions[2].write && transactions[3].write);
        }
    }
}
static void notify_suspend_interleave(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
      for (unsigned int variant = 0; variant < (model == 2 ? 3U : 1U); ++variant) {
        struct led_classdev *led = init_model(model);
        led->brightness = 2;
        ec_ram[UW_EC_REG_KBD_BL_STATUS] = variant ? 2 << 5 : 0;
        struct operation notify = { .kind = OP_NOTIFY };
        struct operation suspend = { .kind = OP_SUSPEND, .led = led };
        pthread_t a, b;
        schedule_active = true;
        /* Also cover same-brightness RGB compensation before and after its
         * complete EC RMW. Each guard has an observable dependent effect. */
        pause_any_ec_unlock = variant != 2;
        pause_call = variant == 2 ? 3 : 0;
        assert(!pthread_create(&a, NULL, first_perform, &notify));
        wait_flag(&first_paused);
        assert(!pthread_create(&b, NULL, perform, &suspend));
        bool blocked = wait_second_boundary();
        assert(READ_ONCE(led->flags) & LED_SUSPENDED);
        release_paused();
        assert(!pthread_join(a, NULL));
        assert(!pthread_join(b, NULL));
        assert(blocked && notify.result && led->brightness == 2 && !hardware_events);
        assert(ec_calls == (model == 2 ? (variant == 2 ? 10 : 8) : 4));
        led_classdev_resume(led);
        flush_work(&led->set_brightness_work);
        assert(led->brightness == 2 && (ec_ram[UW_EC_REG_KBD_BL_STATUS] >> 5) == 2);
      }
    }
}
static void rgb_sequence(unsigned int start, unsigned int red, unsigned int green,
                         unsigned int blue, unsigned int brightness)
{
    expected(start, true, UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS, red * 50 / 255);
    expected(start + 1, true, UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS, green * 50 / 255);
    expected(start + 2, true, UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS, blue * 50 / 255);
    expected(start + 3, false, UW_EC_REG_KBD_BL_RGB_MODE, 0);
    assert(transactions[start + 4].write && transactions[start + 4].address == UW_EC_REG_KBD_BL_RGB_MODE);
    expected(start + 5, false, UW_EC_REG_KBD_BL_STATUS, 0);
    assert(transactions[start + 6].write && transactions[start + 6].address == UW_EC_REG_KBD_BL_STATUS);
    assert((transactions[start + 6].value >> 5) == brightness);
}
static void rgb_transaction(unsigned int direction)
{
    struct led_classdev *led = init_model(2);
    led->brightness = 2;
    uw_mcled_cdev_subleds[0].intensity = 255;
    uw_mcled_cdev_subleds[1].intensity = 127;
    uw_mcled_cdev_subleds[2].intensity = 5;
    /* Independent constant inputs avoid racing the LED core intensity cache. */
    struct mc_subled other_colors[] = { { .intensity = 17 }, { .intensity = 99 }, { .intensity = 231 } };
    struct led_classdev_mc other = uniwill_mcled_cdev;
    other.subled_info = other_colors;
    memset(&other.led_cdev.set_brightness_work, 0, sizeof(other.led_cdev.set_brightness_work));
    struct operation a_op = { .kind = direction == 1 ? OP_RESTORE : OP_CALLBACK,
        .led = direction == 2 ? &other.led_cdev : led, .value = direction == 2 ? 3 : 2 };
    struct operation b_op = { .kind = direction == 2 ? OP_RESTORE : OP_CALLBACK, .led = &other.led_cdev, .value = 3 };
    pthread_t a, b;
    schedule_active = pause_any_ec_unlock = true;
    assert(!pthread_create(&a, NULL, first_perform, &a_op));
    wait_flag(&first_paused);
    assert(!pthread_create(&b, NULL, perform, &b_op));
    bool blocked = wait_second_boundary();
    release_paused();
    assert(!pthread_join(a, NULL));
    assert(!pthread_join(b, NULL));
    assert(blocked && !a_op.result && !b_op.result);
    assert(ec_calls == 14);
    if (direction == 2) {
        rgb_sequence(0, 17, 99, 231, 3);
        rgb_sequence(7, 255, 127, 5, 2);
    } else {
        rgb_sequence(0, 255, 127, 5, 2);
        rgb_sequence(7, 17, 99, 231, 3);
    }
}
static void failure_recovery(bool rgb)
{
    const int errors[] = { -EIO, -ENODEV, -ETIMEDOUT, -EREMOTEIO, -EINVAL };
    for (unsigned int e = 0; e < sizeof(errors) / sizeof(errors[0]); ++e) {
        for (int fail = 1; fail <= (rgb ? 7 : 4); ++fail) {
            struct led_classdev *led = init_model(rgb ? 2 : 1);
            fail_call = fail;
            failure_errno = errors[e];
            assert(invoke(led, 2) == errors[e]);
            assert(ec_calls == fail);
            fail_call = 0;
            int before = ec_calls;
            assert(invoke(led, 3) == 0);
            assert(ec_calls > before && (ec_ram[UW_EC_REG_KBD_BL_STATUS] >> 5) == 3);
        }
    }
}
static void restore_black(bool errors)
{
    for (unsigned int level = 0; level <= 4; ++level) {
        for (unsigned int byte = 0; byte < 256; ++byte) {
            for (int fail = errors ? 1 : 0; fail <= (errors ? 2 : 0); ++fail) {
                struct led_classdev *led = init_model(2);
                for (unsigned int i = 0; i < 3; ++i)
                    uw_mcled_cdev_subleds[i].intensity = 0;
                led->brightness = level;
                ec_ram[UW_EC_REG_KBD_BL_STATUS] = byte;
                fail_call = fail;
                failure_errno = -EREMOTEIO;
                uniwill_leds_restore_state_extern();
                assert(ec_calls == (errors ? fail : 2));
                expected(0, false, UW_EC_REG_KBD_BL_STATUS, 0);
                if (!errors)
                    expected(1, true, UW_EC_REG_KBD_BL_STATUS, status_value(byte, 0));
                assert(led->brightness == level && !hardware_events);
                assert(ec_ram[UW_EC_REG_KBD_BL_STATUS] == (errors ? byte : status_value(byte, 0)));
                fail_call = 0;
                int before = ec_calls;
                uniwill_leds_restore_state_extern();
                assert(ec_calls == before + 2 && led->brightness == level);
            }
        }
    }
}
static void restore_color_errors(void)
{
    for (int fail = 1; fail <= 7; ++fail) {
        struct led_classdev *led = init_model(2);
        led->brightness = 2;
        ec_ram[UW_EC_REG_KBD_BL_STATUS] = 0x0b;
        fail_call = fail;
        failure_errno = -EREMOTEIO;
        uniwill_leds_restore_state_extern();
        /* Preserve the existing independent brightness restoration attempt. */
        assert(ec_calls == (fail <= 5 ? fail + 2 : fail));
        assert(led->brightness == 2 && !hardware_events);
        fail_call = 0;
        int before = ec_calls;
        uniwill_leds_restore_state_extern();
        assert(ec_calls == before + 7 && led->brightness == 2);
    }
}
static void remove_flush(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        struct led_classdev *led = init_model(model);
        led->brightness = 2;
        unregister_pause = schedule_active = true;
        first_thread = pthread_self();
        struct operation remove = { .kind = OP_REMOVE };
        pthread_t remover;
        assert(!pthread_create(&remover, NULL, perform, &remove));
        wait_flag(&unregister_started);
        assert(!uw_leds_initialized);
        assert(!uniwill_leds_notify_brightness_change_extern());
        uniwill_leds_restore_state_extern();
        assert(!ec_calls && !hardware_events);
#if HAS_LED_LOCK
        mutex_lock(&uniwill_led_lock);
#endif
        assert(!pthread_mutex_lock(&schedule_lock));
        allow_unregister = true;
        signal_schedule();
        assert(!pthread_mutex_unlock(&schedule_lock));
        bool blocked = wait_second_boundary();
        assert(!second_done && "unregister must wait for its actual OFF callback");
#if HAS_LED_LOCK
        mutex_unlock(&uniwill_led_lock);
#endif
        assert(!pthread_join(remover, NULL));
        assert(blocked && !remove.result && !registered_led);
        assert((ec_ram[UW_EC_REG_KBD_BL_STATUS] >> 5) == 0);
        assert(ec_calls == (model == 2 ? 7 : 3));
    }
}
static void removed_exports(void)
{
    for (unsigned int model = 0; model < 3; ++model) {
        init_model(model);
        assert(!uniwill_leds_remove(&parent_device));
        int before = ec_calls;
        assert(!uniwill_leds_notify_brightness_change_extern());
        uniwill_leds_restore_state_extern();
        (void)uniwill_leds_get_backlight_type_extern();
        assert(ec_calls == before && !hardware_events);
        assert(!uniwill_leds_remove(&parent_device));
        assert(ec_calls == before);
    }
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    /* Compile the whole frozen core/PM excerpt; the prior runtime fixture
     * exercises its timer and device hierarchy entry points separately. */
    (void)led_timer_function;
    (void)led_suspend;
    (void)led_resume;
    initial_white_flags = uniwill_led_cdev.flags;
    initial_rgb_flags = uniwill_mcled_cdev.led_cdev.flags;
    if (!strcmp(argv[1], "notify_read_errors")) notify_read_errors();
    else if (!strcmp(argv[1], "notify_compensation_errors")) notify_compensation_errors();
    else if (!strcmp(argv[1], "notify_success")) notify_success();
    else if (!strcmp(argv[1], "notify_gate")) notify_gate();
    else if (!strcmp(argv[1], "notify_suspended")) notify_suspended();
    else if (!strcmp(argv[1], "notify_suspend_interleave")) notify_suspend_interleave();
    else if (!strcmp(argv[1], "status_brightness_first")) status_interleave(false);
    else if (!strcmp(argv[1], "status_parent_first")) status_interleave(true);
    else if (!strcmp(argv[1], "rgb_transaction")) rgb_transaction(0);
    else if (!strcmp(argv[1], "restore_transaction")) rgb_transaction(1);
    else if (!strcmp(argv[1], "callback_restore_transaction")) rgb_transaction(2);
    else if (!strcmp(argv[1], "white_failure_recovery")) failure_recovery(false);
    else if (!strcmp(argv[1], "rgb_failure_recovery")) failure_recovery(true);
    else if (!strcmp(argv[1], "restore_black")) restore_black(false);
    else if (!strcmp(argv[1], "restore_black_errors")) restore_black(true);
    else if (!strcmp(argv[1], "restore_color_errors")) restore_color_errors();
    else if (!strcmp(argv[1], "remove_flush")) remove_flush();
    else if (!strcmp(argv[1], "removed_exports")) removed_exports();
    else assert(false && "unknown case");
    return 0;
}
