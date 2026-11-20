/* Minimal process-local kernel API doubles for the driver behavior tests. */
#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <time.h>
#include <unistd.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef int64_t s64;
typedef unsigned short umode_t;
typedef size_t acpi_size;
typedef int acpi_status;
typedef void *acpi_handle;
typedef char *acpi_string;

#define U8_MAX UINT8_MAX
#define U16_MAX UINT16_MAX
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]))
#define __init
#define __always_unused __attribute__((unused))
#define KERNEL_VERSION(a, b, c) (((a) << 16) | ((b) << 8) | (c))
#define LINUX_VERSION_CODE KERNEL_VERSION(7, 3, 5)
#define GFP_KERNEL 0
#define EXPORT_SYMBOL(...)
#define MODULE_ALIAS(...)
#define pr_debug(...) ((void)0)
#define pr_info(...) ((void)0)
#define pr_err(...) ((void)0)
#define READ_ONCE(x) __atomic_load_n(&(x), __ATOMIC_ACQUIRE)
#define WRITE_ONCE(x, v) __atomic_store_n(&(x), (v), __ATOMIC_RELEASE)
#define msecs_to_jiffies(x) (x)

struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(name) struct mutex name = { PTHREAD_MUTEX_INITIALIZER }
#define DEFINE_SPINLOCK(name) DEFINE_MUTEX(name)
static void mutex_init(struct mutex *m) { assert(!pthread_mutex_init(&m->native, NULL)); }
static void mutex_lock(struct mutex *m) { assert(!pthread_mutex_lock(&m->native)); }
static void mutex_unlock(struct mutex *m) { assert(!pthread_mutex_unlock(&m->native)); }
#define spin_lock_irqsave(lock, flags) do { (flags) = 0; mutex_lock(lock); } while (0)
#define spin_unlock_irqrestore(lock, flags) do { (void)(flags); mutex_unlock(lock); } while (0)

struct device;
struct kobject { struct device *dev; };
struct device { void *drvdata; struct kobject kobj; struct device *parent; };
struct platform_device { struct device dev; };
struct wmi_device { struct device dev; };
struct attribute { umode_t mode; };
struct device_attribute { struct attribute attr; };
struct attribute_group { struct attribute **attrs; };
static void *dev_get_drvdata(const struct device *dev) { return dev->drvdata; }
static void dev_set_drvdata(struct device *dev, void *data) { dev->drvdata = data; }
static void *platform_get_drvdata(const struct platform_device *pdev) { return pdev->dev.drvdata; }
static struct device *kobj_to_dev(struct kobject *kobj) { return kobj->dev; }
static int sysfs_emit(char *buffer, const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int count = vsnprintf(buffer, 4096, format, args);
    va_end(args);
    return count;
}
static bool sysfs_streq(const char *a, const char *b)
{
    size_t length = strlen(a);
    if (length && a[length - 1] == '\n')
        --length;
    return strlen(b) == length && !strncmp(a, b, length);
}
static int kstrtou8(const char *buffer, unsigned int base, u8 *out)
{
    char *end;
    errno = 0;
    unsigned long result = strtoul(buffer, &end, base);
    if (end == buffer || errno || result > UINT8_MAX ||
        (*end && !(*end == '\n' && !end[1])))
        return -EINVAL;
    *out = result;
    return 0;
}
static bool allocation_fails;
static void *devm_kzalloc(struct device *dev, size_t size, int flags)
{
    return allocation_fails ? NULL : calloc(1, size);
}
static int group_result, group_calls;
static int devm_device_add_group(struct device *dev, const struct attribute_group *group)
{
    ++group_calls;
    return group_result;
}
#define DMI_SYS_VENDOR 1
#define DMI_PRODUCT_SKU 2
static bool dmi_match(int field, const char *value) { return false; }

#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_BUFFER 3
#define ACPI_ALLOCATE_BUFFER ((size_t)-1)
#define ACPI_FAILURE(status) ((status) != 0)
struct acpi_buffer { size_t length; void *pointer; };
union acpi_object {
    u32 type;
    struct { u32 type; u64 value; } integer;
    struct { u32 type; u32 length; u8 *pointer; } buffer;
};
struct acpi_object_list { u32 count; union acpi_object *pointer; };
static int freed_objects;
static void kfree(void *pointer)
{
    if (pointer)
        ++freed_objects;
    free(pointer);
}

enum tuxi_fan_type { GENERAL = 0, CPU = 1, GPU = 2 };
enum tuxi_fan_mode { AUTO = 0, MANUAL = 1 };
enum hwmon_sensor_types { hwmon_temp, hwmon_fan };
enum { hwmon_temp_input, hwmon_temp_label };
enum { hwmon_fan_input, hwmon_fan_label, hwmon_fan_min, hwmon_fan_max };
struct dmi_system_id { const char *ident; };
enum led_brightness { LED_OFF = 0, LED_FULL = 255 };
struct led_classdev {
    const char *name;
    enum led_brightness brightness;
    enum led_brightness max_brightness;
    int flags;
    void (*brightness_set)(struct led_classdev *, enum led_brightness);
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
};
#define LED_FUNCTION_KBD_BACKLIGHT "kbd_backlight"
#define LED_BRIGHT_HW_CHANGED 1
#define IFLX14I01 "TUXEDO InfinityFlex 14 Gen1"
