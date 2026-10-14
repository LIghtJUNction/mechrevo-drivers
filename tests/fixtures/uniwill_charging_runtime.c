/* Real charging, sysfs and platform-remove functions; no hardware access. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

typedef uint8_t u8;
typedef uint16_t u16;
struct device { struct { int unused; } kobj; };
struct platform_device { struct device dev; };
struct device_attribute { int unused; };
struct uniwill_device_features_t { bool uniwill_has_charging_prio, uniwill_has_charging_profile; };
static struct uniwill_device_features_t uniwill_device_features;
struct char_to_u8_t { char *descriptor; u8 value; };
static int uw_charging_prio_attr_group, uw_charging_profile_attr_group;
static bool uw_lightbar_loaded, uniwill_fan_removed;
static u8 uniwill_kbd_bl_enable_state_on_start = 0xff;
static struct platform_device platform;
static int errors, last_error;
#define TUXEDO_ERROR(format, error) do { (void)(format); ++errors; last_error = (error); } while (0)
#define LINUX_VERSION_CODE 0x070209
#define KERNEL_VERSION(a,b,c) (((a)<<16)|((b)<<8)|(c))
#define GFP_KERNEL 0
#define ARRAY_SIZE(a) ((int)(sizeof(a)/sizeof((a)[0])))
#define UW_EC_REG_USB_POWERSHARE_STATUS 0x0767
/* @HAS_VALID@ */
/* @ACTUAL_EC@ */
/* @HAS_UPDATE@ */

static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static _Thread_local int actor;
static _Thread_local int charging_depth;
static bool done[4], waiting[4], published[2];
static int active_store;
static bool removing;
struct mutex { pthread_mutex_t native; bool charging; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER, false }
static DEFINE_MUTEX(uniwill_charging_lock);
static DEFINE_MUTEX(uniwill_fan_command_lock);
static DEFINE_MUTEX(uniwill_fan_state_lock);
static void mutex_lock(struct mutex *lock)
{
    int status = pthread_mutex_trylock(&lock->native);
    if (status == EBUSY) {
        pthread_mutex_lock(&harness);
        waiting[actor] = true;
        pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&harness);
        status = pthread_mutex_lock(&lock->native);
    }
    assert(status == 0);
    if (lock == &uniwill_charging_lock) ++charging_depth;
}
static void mutex_unlock(struct mutex *lock)
{
    if (lock == &uniwill_charging_lock) --charging_depth;
    assert(pthread_mutex_unlock(&lock->native) == 0);
}
static u8 ec[UINT16_MAX + 1u];
static int reads, writes, create_error;
static u16 fault_address;
static char fault_kind;
static int fault_error, faults;
static bool poison_failed_read;
static u16 pause_address;
static char pause_kind;
static bool paused, release_pause;
static int pause_actor = 1;
static bool allocation_fail;
#if !ACTUAL_EC
static pthread_mutex_t ec_transaction = PTHREAD_MUTEX_INITIALIZER;
#endif
static void pause_selected(char kind, u16 address)
{
    if (actor == pause_actor && kind == pause_kind && address == pause_address) {
        paused = true;
        pthread_cond_broadcast(&changed);
        while (!release_pause) pthread_cond_wait(&changed, &harness);
    }
}
static int mock_read_ec_ram(u16 address, u8 *value)
{
    pthread_mutex_lock(&harness);
    ++reads;
    bool failed = faults && fault_kind == 'r' && fault_address == address;
    if (failed) --faults;
    if (!failed) *value = ec[address];
    else if (poison_failed_read) *value = 0xff;
    pause_selected('r', address); /* after sampling, before returning */
    pthread_mutex_unlock(&harness);
    return failed ? fault_error : 0;
}
static int mock_write_ec_ram(u16 address, u8 value)
{
    pthread_mutex_lock(&harness);
    ++writes;
    bool failed = faults && fault_kind == 'w' && fault_address == address;
    if (failed) --faults;
    if (!failed) ec[address] = value;
    pause_selected('w', address); /* after committing, before returning */
    pthread_mutex_unlock(&harness);
    return failed ? fault_error : 0;
}
#if ACTUAL_EC
struct uniwill_interface_t { int (*read_ec_ram)(u16, u8 *); int (*write_ec_ram)(u16, u8); };
static struct uniwill_interface_t transport = { mock_read_ec_ram, mock_write_ec_ram };
static struct { struct uniwill_interface_t *wmi; } uniwill_interfaces = { &transport };
static DEFINE_MUTEX(uniwill_ec_transaction_lock);
#define IS_ERR_OR_NULL(pointer) (!(pointer))
#define pr_err(...) do { ++errors; } while (0)
#define EXPORT_SYMBOL(name)
/* @EC_FUNCTIONS@ */
#else
#define uniwill_read_ec_ram mock_read_ec_ram
#define uniwill_write_ec_ram mock_write_ec_ram
/* Mock the shared-EC API to expose the precise post-transaction cache window. */
static int uniwill_update_ec_ram(u16 address, u8 clear, u8 set)
{
    int status = pthread_mutex_trylock(&ec_transaction);
    if (status == EBUSY) {
        pthread_mutex_lock(&harness); waiting[actor] = true;
        pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
        status = pthread_mutex_lock(&ec_transaction);
    }
    assert(status == 0);
    u8 value;
    int result = uniwill_read_ec_ram(address, &value);
    if (!result) result = uniwill_write_ec_ram(address, (value & ~clear) | set);
    assert(pthread_mutex_unlock(&ec_transaction) == 0);
    /* Isolate the cache window from the lower EC mutex. A competing setter
     * must still wait for charging after this hardware transaction ended. */
    pthread_mutex_lock(&harness); pause_selected('u', address); pthread_mutex_unlock(&harness);
    return result;
}
#endif
static void *kmalloc(size_t size, int flags) __attribute__((unused));
static void *kmalloc(size_t size, int flags) { (void)flags; return allocation_fail ? NULL : malloc(size); }
static char *kmemdup_nul(const void *input, size_t size, int flags) __attribute__((unused));
static char *kmemdup_nul(const void *input, size_t size, int flags)
{
    char *copy = kmalloc(size + 1, flags);
    if (copy) { memcpy(copy, input, size); copy[size] = '\0'; }
    return copy;
}
#define kfree free
static char *strstrip(char *text)
{
    while (isspace((unsigned char)*text)) ++text;
    size_t length = strlen(text);
    while (length && isspace((unsigned char)text[length - 1])) text[--length] = '\0';
    return text;
}
static int sysfs_create_group(void *kobj, void *group)
{
    (void)kobj;
    assert(charging_depth == 0); /* cleanup must be able to drain stores */
    pthread_mutex_lock(&harness);
    if (!create_error) published[group == &uw_charging_profile_attr_group] = true;
    pthread_cond_broadcast(&changed);
    pause_selected('p', group == &uw_charging_profile_attr_group ? 0x07a6 : 0x07cc);
    pthread_mutex_unlock(&harness);
    return create_error;
}
static void sysfs_remove_group(void *kobj, void *group)
{
    (void)kobj;
    assert(charging_depth == 0);
    pthread_mutex_lock(&harness);
    published[group == &uw_charging_profile_attr_group] = false;
    removing = true;
    pthread_cond_broadcast(&changed);
    while (active_store) pthread_cond_wait(&changed, &harness);
    pthread_mutex_unlock(&harness);
}
static void uniwill_keyboard_prepare_remove(void) {}
static void uw_fan_quiesce_locked(void) {}
static void uw_battery_uninit(void) {}
static void uniwill_leds_remove(struct platform_device *dev) { (void)dev; }
static int uniwill_write_kbd_bl_enable(u8 value) { (void)value; return 0; }
static void uw_lightbar_remove(struct platform_device *dev) { (void)dev; }
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop

static bool profile;
static u16 address(void) { return profile ? 0x07a6 : 0x07cc; }
static u8 cache(void) { return profile ? uw_charging_profile_last_written_value : uw_charging_prio_last_written_value; }
static bool valid(void)
{
#if HAS_VALID
    return profile ? uw_charging_profile_state_valid : uw_charging_prio_state_valid;
#else
    return true;
#endif
}
static bool loaded(void) { return profile ? uw_charging_profile_loaded : uw_charging_prio_loaded; }
static void init(void)
{
    if (profile) uw_charging_profile_init(&platform);
    else uw_charging_priority_init(&platform);
}
static void restore(void)
{
    if (profile) uw_charging_profile_write_state();
    else uw_charging_priority_write_state();
}
static int set(u8 value)
{
    return profile ? uw_set_charging_profile(value) : uw_set_charging_priority(value);
}
static ssize_t store(const char *text, size_t size)
{
    return profile ? uw_charging_profile_store(&platform.dev, NULL, text, size)
                   : uw_charging_prio_store(&platform.dev, NULL, text, size);
}
static const char *request(u8 value)
{
    if (profile) return value == 0 ? "high_capacity" : value == 1 ? "balanced" : "stationary";
    return value ? "performance" : "charge_battery";
}
static u8 encoded(u8 value) { return profile ? (value & 3) << 4 : (value & 1) << 7; }
static u8 mask(void) { return profile ? 0x30 : 0x80; }
static void reset(bool selected)
{
    profile = selected;
    memset(ec, 0, sizeof(ec));
    reads = writes = errors = last_error = create_error = faults = 0;
    fault_kind = pause_kind = 0; paused = release_pause = false;
    poison_failed_read = allocation_fail = false;
    memset(done, 0, sizeof(done)); memset(waiting, 0, sizeof(waiting));
    memset(published, 0, sizeof(published)); active_store = 0; removing = false;
    uw_charging_prio_loaded = uw_charging_profile_loaded = false;
    uw_charging_prio_last_written_value = 0; uw_charging_profile_last_written_value = 0;
#if HAS_VALID
    uw_charging_prio_state_valid = uw_charging_profile_state_valid = false;
#endif
    uniwill_device_features.uniwill_has_charging_prio = true;
    uniwill_device_features.uniwill_has_charging_profile = true;
    actor = 0;
}
static void fault(char kind, int error)
{
    fault_kind = kind; fault_address = address(); fault_error = error; faults = 1;
}
static void begin(void) { ec[address()] = 0x85 | encoded(1); init(); reads = writes = 0; }
enum operation { SET, RESTORE, INIT, REMOVE, USB, RGB };
static enum operation op[4];
static u8 values[4];
static void *run(void *arg)
{
    actor = (int)(intptr_t)arg;
    if (op[actor] == SET) {
        pthread_mutex_lock(&harness); ++active_store; pthread_mutex_unlock(&harness);
        const char *text = request(values[actor]);
        assert(store(text, strlen(text)) == (ssize_t)strlen(text));
        pthread_mutex_lock(&harness); --active_store; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
    } else if (op[actor] == RESTORE) restore();
    else if (op[actor] == INIT) init();
    else if (op[actor] == REMOVE) uniwill_keyboard_remove(&platform);
    else if (op[actor] == USB) assert(uw_set_usb_powershare(1) == 0);
    else assert(uniwill_update_ec_ram(0x0767, 0, 0x20) == 0);
    pthread_mutex_lock(&harness); done[actor] = true;
    pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
    return NULL;
}
static pthread_t launch(int id, enum operation operation, u8 value)
{
    pthread_t thread;
    op[id] = operation; values[id] = value;
    assert(pthread_create(&thread, NULL, run, (void *)(intptr_t)id) == 0);
    return thread;
}
static void wait_paused(void)
{
    pthread_mutex_lock(&harness);
    while (!paused) pthread_cond_wait(&changed, &harness);
    pthread_mutex_unlock(&harness);
}
static void wait_other(void)
{
    pthread_mutex_lock(&harness);
    while (!waiting[2] && !done[2]) pthread_cond_wait(&changed, &harness);
    pthread_mutex_unlock(&harness);
}
static void release(void)
{
    pthread_mutex_lock(&harness); release_pause = true;
    pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
}
static void join(pthread_t thread) { assert(pthread_join(thread, NULL) == 0); }
static void concurrent(enum operation first, enum operation second)
{
    begin(); pause_kind = HAS_UPDATE ? 'u' : 'w'; pause_address = address();
    pthread_t a = launch(1, first, 0); wait_paused();
    /* Priority restore starts at 1, so its competing setter must request 0. */
    u8 requested = first == RESTORE && !profile ? 0 : (profile ? 2 : 1);
    pthread_t b = launch(2, second, requested); wait_other();
    release(); join(a); join(b);
    u8 expected = second == SET ? requested : 0;
    assert((ec[address()] & mask()) == encoded(expected));
    assert(cache() == expected);
    ec[address()] ^= mask(); restore();
    assert((ec[address()] & mask()) == encoded(expected));
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *name = argv[1];
    reset(strncmp(name, "profile_", 8) == 0);
    if (!strcmp(name, "priority_bytes") || !strcmp(name, "profile_bytes")) {
        for (int previous = 0; previous < 256; ++previous)
            for (int value = 0; value < 256; ++value) {
                ec[address()] = previous;
                assert(set(value) == 0);
                assert(ec[address()] == ((previous & ~mask()) | encoded(value)));
                assert(cache() == (value & (profile ? 3 : 1))); assert(valid());
            }
    } else if (!strcmp(name, "get_bytes")) {
        for (int kind = 0; kind < 2; ++kind) for (int previous = 0; previous < 256; ++previous) {
            profile = kind; ec[address()] = previous; u8 output = 0xa5;
            int result = profile ? uw_get_charging_profile(&output) : uw_get_charging_priority(&output);
            assert(result == 0); assert(output == ((previous & mask()) >> (profile ? 4 : 7)));
        }
    } else if (!strcmp(name, "get_errors")) {
        int err[] = {-EIO, -ETIMEDOUT, -ENODEV, -EAGAIN};
        for (int kind = 0; kind < 2; ++kind) {
            profile = kind;
            for (unsigned i = 0; i < sizeof(err)/sizeof(err[0]); ++i) {
                u8 value = 0xa5; poison_failed_read = true; fault('r', err[i]);
                int result = profile ? uw_get_charging_profile(&value) : uw_get_charging_priority(&value);
                assert(result == err[i]); assert(value == 0xa5);
            }
        }
    } else if (!strcmp(name, "set_errors")) {
        int err[] = {-EIO, -ETIMEDOUT, -ENODEV, -EAGAIN};
        for (int kind = 0; kind < 2; ++kind) for (int rw = 0; rw < 2; ++rw)
            for (unsigned i = 0; i < sizeof(err)/sizeof(err[0]); ++i) {
                reset(kind); begin(); u8 previous = ec[address()], saved = cache();
                fault(rw ? 'w' : 'r', err[i]); assert(set(0) == err[i]);
                assert(ec[address()] == previous && cache() == saved && valid());
                assert(writes == rw);
            }
    } else if (strstr(name, "_setters")) concurrent(SET, SET);
    else if (strstr(name, "_restore_first")) concurrent(RESTORE, SET);
    else if (strstr(name, "_set_first")) concurrent(SET, RESTORE);
    else if (strstr(name, "_init_failure")) {
        poison_failed_read = true; fault('r', -ETIMEDOUT); init();
        assert(loaded());
        reads = writes = 0; restore(); assert(reads == 0 && writes == 0);
        assert(!valid()); assert(errors == 1 && last_error == -ETIMEDOUT);
    } else if (!strcmp(name, "init_recovery")) {
        for (int kind = 0; kind < 2; ++kind) {
            reset(kind); fault('r', -EIO); init();
            assert(loaded()); reads = writes = 0; restore(); assert(reads == 0 && writes == 0);
            assert(!valid()); const char *text = request(1);
            assert(store(text, strlen(text)) == (ssize_t)strlen(text)); assert(valid());
            ec[address()] = 0x45 & ~mask(); restore();
            assert(ec[address()] == ((0x45 & ~mask()) | encoded(1)));
        }
    } else if (!strcmp(name, "invalid_set_failure")) {
        for (int kind = 0; kind < 2; ++kind) for (int rw = 0; rw < 2; ++rw) {
            reset(kind); fault('r', -EIO); init(); assert(loaded());
            fault(rw ? 'w' : 'r', -ETIMEDOUT); assert(set(1) == -ETIMEDOUT);
            reads = writes = 0; restore(); assert(reads == 0 && writes == 0); assert(!valid());
        }
    } else if (!strcmp(name, "publication_window")) {
        for (int kind = 0; kind < 2; ++kind) {
            reset(kind); ec[address()] = encoded(1); pause_kind = 'p'; pause_address = address();
            pthread_t a = launch(1, INIT, 0); wait_paused();
            pthread_t b = launch(2, SET, 0); join(b); release(); join(a);
            assert(cache() == 0 && loaded() && valid());
            ec[address()] = encoded(1); restore(); assert((ec[address()] & mask()) == 0);
        }
    } else if (strstr(name, "_publication")) {
        ec[address()] = encoded(1); pause_kind = 'r'; pause_address = address();
        pthread_t a = launch(1, INIT, 0); wait_paused();
        pthread_mutex_lock(&harness); bool already_published = published[profile]; pthread_mutex_unlock(&harness);
        if (!already_published) {
            release(); join(a);
            pthread_t b = launch(2, SET, 0); join(b);
        } else {
            pthread_t b = launch(2, SET, 0); join(b); release(); join(a);
        }
        assert(cache() == 0); ec[address()] = encoded(1); restore();
        assert((ec[address()] & mask()) == 0);
    } else if (!strcmp(name, "registration_rebind")) {
        for (int kind = 0; kind < 2; ++kind) {
            reset(kind); begin(); create_error = -ENOMEM; init();
            assert(!loaded()); reads = writes = 0; restore(); assert(reads == 0 && writes == 0);
#if HAS_VALID
            assert(!valid());
#endif
            create_error = 0; init(); assert(loaded() && valid());
            uniwill_keyboard_remove(&platform); reads = writes = 0; restore();
            assert(reads == 0 && writes == 0); assert(!loaded() && !valid());
            if (profile) uniwill_device_features.uniwill_has_charging_profile = false;
            else uniwill_device_features.uniwill_has_charging_prio = false;
            reads = writes = 0; init(); restore(); assert(!loaded() && !valid()); assert(reads == 0 && writes == 0);
        }
    } else if (!strcmp(name, "remove_drain")) {
        begin(); pause_kind = 'w'; pause_address = address();
        pthread_t a = launch(1, SET, 0); wait_paused();
        pthread_t b = launch(2, REMOVE, 0);
        pthread_mutex_lock(&harness); while (!removing) pthread_cond_wait(&changed, &harness); pthread_mutex_unlock(&harness);
        release(); join(a); join(b);
        reads = writes = 0; restore(); assert(reads == 0 && writes == 0); assert(!loaded() && !valid());
    } else if (!strcmp(name, "restore_retry")) {
        for (int kind = 0; kind < 2; ++kind) for (int rw = 0; rw < 2; ++rw) {
            reset(kind); begin(); ec[address()] &= ~mask();
            fault(rw ? 'w' : 'r', -ETIMEDOUT); restore();
            assert(errors == 1 && last_error == -ETIMEDOUT && valid() && cache() == 1);
            restore(); assert((ec[address()] & mask()) == encoded(1));
        }
    } else if (strstr(name, "_alloc")) {
        allocation_fail = true; const char *text = request(1);
        assert(store(text, strlen(text)) == -ENOMEM); assert(reads == 0 && writes == 0);
    } else if (!strcmp(name, "parse")) {
        for (int kind = 0; kind < 2; ++kind) {
            reset(kind);
            for (int value = 0; value <= (profile ? 2 : 1); ++value) {
                char text[64]; snprintf(text, sizeof(text), " \t%s\r\n ", request(value));
                assert(store(text, strlen(text)) == (ssize_t)strlen(text)); assert(cache() == value);
            }
            int before = reads + writes;
            assert(store("", 0) == -EINVAL); assert(store(" \t\n", 3) == -EINVAL);
            assert(store("unknown", 7) == -EINVAL); assert(reads + writes == before);
            fault('r', -ETIMEDOUT); assert(store(request(0), strlen(request(0))) == -EIO);
            fault('w', -ENODEV); assert(store(request(0), strlen(request(0))) == -EIO);
        }
    } else if (!strcmp(name, "bounded")) {
        profile = true; assert(store("balancedTRAILING", 8) == 8); assert(cache() == 1);
        profile = false; assert(store("performanceTRAILING", 11) == 11); assert(cache() == 1);
    } else if (!strcmp(name, "usb_bytes")) {
        for (int previous = 0; previous < 256; ++previous)
            for (int value = 0; value < 256; ++value) {
                ec[0x0767] = previous; assert(uw_set_usb_powershare(value) == 0);
                assert(ec[0x0767] == ((previous & ~0x10) | ((value & 1) << 4)));
                assert(uw_usb_powershare_last_written_value == (bool)(value & 1));
            }
        int err[] = {-EIO, -ETIMEDOUT, -ENODEV};
        for (int rw = 0; rw < 2; ++rw) for (unsigned i = 0; i < sizeof(err)/sizeof(err[0]); ++i) {
            fault_kind = rw ? 'w' : 'r'; fault_address = 0x0767; fault_error = err[i]; faults = 1;
            u8 previous = ec[0x0767]; bool saved = uw_usb_powershare_last_written_value;
            assert(uw_set_usb_powershare(0) == err[i]);
            assert(ec[0x0767] == previous && uw_usb_powershare_last_written_value == saved);
        }
    } else if (!strcmp(name, "usb_rgb")) {
        ec[0x0767] = 0x80; pause_kind = 'r'; pause_address = 0x0767;
        pthread_t a = launch(1, USB, 0); wait_paused();
        pthread_t b = launch(2, RGB, 0); wait_other(); release(); join(a); join(b);
        assert(ec[0x0767] == 0xb0);
    } else assert(!"Unknown test case");
    return 0;
}
