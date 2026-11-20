#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#define __init
#define __exit
#define KERNEL_VERSION(a, b, c) (((a) << 16) + ((b) << 8) + (c))
#define LINUX_VERSION_CODE KERNEL_VERSION(7, 3, 5)
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define IS_ERR(p) ((uintptr_t)(p) >= (uintptr_t)-4095)
#define IS_ERR_OR_NULL(p) (!(p) || IS_ERR(p))
#define PTR_ERR(p) ((int)(intptr_t)(p))
#define ERR_PTR(e) ((void *)(intptr_t)(e))
#define pr_err(...) do {} while (0)
#define pr_debug(...) do {} while (0)
#define pr_warn(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define TUXEDO_ERROR(...) do {} while (0)
#define DMI_BOARD_NAME 1
#define DMI_PRODUCT_NAME 2
#define DMI_PRODUCT_SKU 3
#define EXPORT_SYMBOL(n)
/* @CONSTANTS@ */
struct device { int unused; };
struct platform_device { struct device dev; };
struct class { int unused; };
struct cdev { int unused; };
static int failure_stage;
static bool region_live, cdev_live, class_live, device_live;
static char operations[32];
static size_t operation_count;
static void operation(char c) { operations[operation_count++] = c; }
static struct class klass;
static struct device device;
static struct cdev tuxedo_io_cdev;
static struct class *tuxedo_io_device_class;
static dev_t tuxedo_io_device_handle;
static int fops_dev;
static u32 id_check_clevo, id_check_uniwill;
static u32 clevo_identify(void) { return 1; }
static u32 uniwill_identify(void) { return 0; }
static int alloc_chrdev_region(dev_t *handle, int first, int count, const char *name)
{
    operation('A');
    if (failure_stage == 1) return -EBUSY;
    region_live = true; *handle = 123; return 0;
}
static void unregister_chrdev_region(dev_t handle, int count)
{ assert(region_live && handle == 123 && !cdev_live); operation('a'); region_live = false; }
static void cdev_init(struct cdev *dev, void *fops) { assert(region_live); }
static int cdev_add(struct cdev *dev, dev_t handle, int count)
{
    assert(region_live); operation('C');
    if (failure_stage == 2) return -EIO;
    cdev_live = true; return 0;
}
static void cdev_del(struct cdev *dev)
{ assert(cdev_live && !class_live); operation('c'); cdev_live = false; }
static struct class *class_create(const char *name)
{
    assert(cdev_live); operation('L');
    if (failure_stage == 3) return ERR_PTR(-ENOMEM);
    class_live = true; return &klass;
}
static void class_destroy(struct class *c)
{ assert(c == &klass && class_live && !device_live); operation('l'); class_live = false; }
static struct device *device_create(struct class *c, void *parent, dev_t handle,
                                    void *data, const char *name)
{
    assert(c == &klass && class_live && cdev_live); operation('D');
    if (failure_stage == 4) return ERR_PTR(-ENODEV);
    device_live = true; return &device;
}
static void device_destroy(struct class *c, dev_t handle)
{ assert(c == &klass && device_live); operation('d'); device_live = false; }
/* @FUNCTION tuxedo_io/tuxedo_io.c tuxedo_io_init@ */
/* @FUNCTION tuxedo_io/tuxedo_io.c tuxedo_io_exit@ */

enum led_brightness { LED_OFF = 0, LED_FULL = 255 };
struct led_classdev {
    const char *name;
    unsigned int max_brightness;
    enum led_brightness brightness;
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
    enum led_brightness (*brightness_get)(struct led_classdev *);
};
static u8 ram[0x800];
static unsigned int reads, writes;
static u16 read_addresses[16], write_addresses[16];
static int read_failure, write_failure;
struct mutex { bool held; };
static struct mutex uniwill_ec_transaction_lock;
static void mutex_lock(struct mutex *m) { assert(!m->held); m->held = true; }
static void mutex_unlock(struct mutex *m) { assert(m->held); m->held = false; }
static int uniwill_read_ec_ram_unlocked(u16 address, u8 *value)
{
    assert(uniwill_ec_transaction_lock.held && reads < 16);
    read_addresses[reads++] = address;
    if (read_failure) return read_failure;
    *value = ram[address]; return 0;
}
static int uniwill_write_ec_ram_unlocked(u16 address, u8 value)
{
    assert(uniwill_ec_transaction_lock.held && writes < 16);
    write_addresses[writes++] = address;
    if (write_failure) return write_failure;
    ram[address] = value; return 0;
}
/* @FUNCTION uniwill_keyboard.h uniwill_read_ec_ram@ */
/* @FUNCTION uniwill_keyboard.h uniwill_write_ec_ram@ */
/* @FUNCTION uniwill_keyboard.h uniwill_update_ec_ram@ */
/* @FUNCTION uniwill_keyboard.h uniwill_write_lightbar_rgb@ */
/* @FUNCTION uniwill_keyboard.h uniwill_write_lightbar_animation@ */
/* @OLD_LIGHTBAR_READERS@ */
/* @FUNCTION uniwill_keyboard.h lightbar_set_blocking@ */
/* @FUNCTION uniwill_keyboard.h lightbar_get@ */
static struct led_classdev lightbar_led_classdevs[] = {
    { .name = UNIWILL_LIGHTBAR_LED_NAME_RGB_RED, .max_brightness = UNIWILL_LIGHTBAR_LED_MAX_BRIGHTNESS },
    { .name = UNIWILL_LIGHTBAR_LED_NAME_RGB_GREEN, .max_brightness = UNIWILL_LIGHTBAR_LED_MAX_BRIGHTNESS },
    { .name = UNIWILL_LIGHTBAR_LED_NAME_RGB_BLUE, .max_brightness = UNIWILL_LIGHTBAR_LED_MAX_BRIGHTNESS },
    { .name = UNIWILL_LIGHTBAR_LED_NAME_ANIMATION, .max_brightness = 1 },
};
static int led_registrations, led_unregistrations, registration_failure;
static int led_classdev_register(struct device *d, struct led_classdev *led)
{
    ++led_registrations;
    return registration_failure == led_registrations ? -ENOMEM : 0;
}
static void led_classdev_unregister(struct led_classdev *led) { ++led_unregistrations; }
static int dmi_match(int field, const char *name)
{ return field == DMI_BOARD_NAME && !strcmp(name, "LAPQC71A"); }
/* @FUNCTION uniwill_keyboard.h uw_lightbar_init@ */

typedef unsigned long efi_status_t;
typedef wchar_t efi_char16_t;
typedef int efi_guid_t;
#define EFI_SUCCESS 0
#define EFI_BUFFER_TOO_SMALL 5
#define EFI_RUNTIME_SERVICES 1
#define GFP_KERNEL 0
static struct uniwill_device_features_t { bool uniwill_has_hidden_bios_options; } uniwill_device_features;
static efi_guid_t uw_oem_magic_guid;
static u8 efi_allocation[128], efi_payload[128];
static unsigned long efi_read_size;
static int efi_reads, efi_writes, efi_allocations, efi_frees;
static bool efi_enabled(int feature) { return true; }
static void *kmalloc(size_t size, int flags)
{
    assert(size == sizeof(efi_allocation));
    ++efi_allocations;
    /* Leftover allocation bytes can look like supported, disabled switches.
     * A short second read must never use these bytes to enable/write them. */
    memset(efi_allocation, 1, sizeof(efi_allocation));
    efi_allocation[UW_MEMORY_OVERCLOCKING_SWITCH] = 0;
    efi_allocation[UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH] = 0;
    return efi_allocation;
}
static void kfree(void *pointer)
{ assert(pointer == efi_allocation && efi_frees < efi_allocations); ++efi_frees; }
static efi_status_t efi_get_variable(efi_char16_t *name, efi_guid_t *guid, u32 *attrs,
                                     unsigned long *size, void *buffer)
{
    ++efi_reads;
    if (!buffer) {
        assert(*size == 0);
        *size = sizeof(efi_allocation); *attrs = 7;
        return EFI_BUFFER_TOO_SMALL;
    }
    assert(buffer == efi_allocation && *size == sizeof(efi_allocation));
    assert(efi_read_size <= *size);
    memcpy(buffer, efi_payload, efi_read_size);
    *size = efi_read_size;
    return EFI_SUCCESS;
}
static efi_status_t efi_set_variable(efi_char16_t *name, efi_guid_t *guid, u32 attrs,
                                     unsigned long size, void *buffer)
{
    ++efi_writes;
    assert(size == efi_read_size && attrs == 7 && buffer == efi_allocation);
    assert(size > UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH);
    assert(efi_allocation[UW_MEMORY_OVERCLOCKING_SWITCH] == 1);
    assert(efi_allocation[UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH] == 1);
    return EFI_SUCCESS;
}
static struct {
    efi_status_t (*get_variable)(efi_char16_t *, efi_guid_t *, u32 *, unsigned long *, void *);
    efi_status_t (*set_variable)(efi_char16_t *, efi_guid_t *, u32, unsigned long, void *);
} efi = { efi_get_variable, efi_set_variable };
/* @FUNCTION uniwill_keyboard.h uw_show_hidden_bios_options@ */

int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *name = argv[1];
    if (!strncmp(name, "io_", 3)) {
        const int errors[] = { 0, -EBUSY, -EIO, -ENOMEM, -ENODEV };
        const char *traces[] = { "ACLDdlca", "A", "ACa", "ACLca", "ACLDlca" };
        if (!strcmp(name, "io_alloc_failure")) failure_stage = 1;
        else if (!strcmp(name, "io_cdev_failure")) failure_stage = 2;
        else if (!strcmp(name, "io_class_failure")) failure_stage = 3;
        else if (!strcmp(name, "io_device_failure")) failure_stage = 4;
        else assert(!strcmp(name, "io_success"));
        assert(tuxedo_io_init() == errors[failure_stage]);
        if (!failure_stage) {
            assert(region_live && cdev_live && class_live && device_live);
            tuxedo_io_exit();
        }
        assert(!region_live && !cdev_live && !class_live && !device_live);
        assert(!strcmp(operations, traces[failure_stage]));
    } else if (!strcmp(name, "lightbar_channel_reads")) {
        ram[0x749] = 10; ram[0x74a] = 20; ram[0x74b] = 30; ram[0x748] = 0x81;
        for (int i = 0; i < 4; ++i) {
            reads = 0;
            assert(lightbar_get(&lightbar_led_classdevs[i]) == (i < 3 ? (i + 1) * 10 : 1));
            assert(reads == 1 && read_addresses[0] == (i < 3 ? 0x749 + i : 0x748));
        }
    } else if (!strcmp(name, "lightbar_read_failure")) {
        for (int i = 0; i < 4; ++i) {
            reads = 0; read_failure = -EIO;
            lightbar_led_classdevs[i].brightness = i == 3 ? 1 : 7;
            assert((int)lightbar_get(&lightbar_led_classdevs[i]) == -EIO);
            assert(lightbar_led_classdevs[i].brightness == (i == 3 ? 1 : 7));
            assert(reads == 1 && !writes);
        }
        read_failure = 0; ram[0x749] = 255;
        assert((int)lightbar_get(&lightbar_led_classdevs[0]) == -ERANGE);
        assert(lightbar_led_classdevs[0].brightness == 7);
    } else if (!strcmp(name, "lightbar_animation_bits")) {
        for (int value = 0; value <= 255; ++value) {
            reads = writes = 0; ram[0x748] = value;
            assert(!lightbar_set_blocking(&lightbar_led_classdevs[3], 1));
            assert(ram[0x748] == (value | 0x80));
            assert(!lightbar_set_blocking(&lightbar_led_classdevs[3], 0));
            assert(ram[0x748] == (value & 0x7f) && reads == 2 && writes == 2);
        }
    } else if (!strcmp(name, "lightbar_animation_failure")) {
        read_failure = -ETIMEDOUT; ram[0x748] = 0x5a;
        assert(lightbar_set_blocking(&lightbar_led_classdevs[3], 1) == -ETIMEDOUT);
        assert(!writes && ram[0x748] == 0x5a);
        read_failure = 0; write_failure = -EIO;
        assert(lightbar_set_blocking(&lightbar_led_classdevs[3], 1) == -EIO);
    } else if (!strcmp(name, "lightbar_color_failure")) {
        for (int i = 0; i < 3; ++i) {
            reads = writes = 0; write_failure = -EIO;
            assert(lightbar_set_blocking(&lightbar_led_classdevs[i], 12) == -EIO);
            assert(writes == 1 && write_addresses[0] == 0x749 + i && !reads);
        }
        writes = reads = 0; write_failure = 0; read_failure = -ETIMEDOUT;
        assert(lightbar_set_blocking(&lightbar_led_classdevs[0], 12) == -ETIMEDOUT);
        assert(writes == 1 && ram[0x749] == 12);
    } else if (!strcmp(name, "lightbar_invalid")) {
        assert(lightbar_set_blocking(&lightbar_led_classdevs[0], 37) == -EINVAL);
        assert(lightbar_set_blocking(&lightbar_led_classdevs[3], 2) == -EINVAL);
        struct led_classdev unknown = { .name = "unrelated", .max_brightness = 1 };
        assert(lightbar_set_blocking(&unknown, 1) == -EINVAL && !reads && !writes);
    } else if (!strcmp(name, "lightbar_init_failure")) {
        struct platform_device pdev;
        read_failure = -ETIMEDOUT;
        assert(uw_lightbar_init(&pdev) == -ETIMEDOUT);
        assert(led_registrations == 4 && led_unregistrations == 4 && !writes);
        led_registrations = led_unregistrations = 0; read_failure = 0; write_failure = -EIO;
        assert(uw_lightbar_init(&pdev) == -EIO);
        assert(led_registrations == 4 && led_unregistrations == 4);
    } else if (!strcmp(name, "lightbar_init_success")) {
        struct platform_device pdev;
        memset(ram, 0x9a, sizeof(ram));
        assert(!uw_lightbar_init(&pdev));
        assert(led_registrations == 4 && !led_unregistrations);
        assert(ram[0x748] == 0x1a && !ram[0x749] && !ram[0x74a] && !ram[0x74b]);
    } else if (!strcmp(name, "efi_short_second_read")) {
        const unsigned long sizes[] = { 0, UW_MEMORY_OVERCLOCKING_SWITCH,
            UW_MEMORY_OVERCLOCKING_SUPPORT, UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SUPPORT,
            UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH };
        uniwill_device_features.uniwill_has_hidden_bios_options = true;
        memset(efi_payload, 1, sizeof(efi_payload));
        efi_payload[UW_MEMORY_OVERCLOCKING_SWITCH] = 0;
        efi_payload[UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH] = 0;
        for (size_t i = 0; i < ARRAY_SIZE(sizes); ++i) {
            efi_read_size = sizes[i];
            uw_show_hidden_bios_options();
            assert(efi_reads == 2 * (i + 1) && !efi_writes);
            assert(efi_allocations == i + 1 && efi_frees == efi_allocations);
            assert(efi_allocation[UW_MEMORY_OVERCLOCKING_SWITCH] == 0);
            assert(efi_allocation[UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH] == 0);
        }
    } else if (!strcmp(name, "efi_minimum_valid_read")) {
        uniwill_device_features.uniwill_has_hidden_bios_options = true;
        memset(efi_payload, 1, sizeof(efi_payload));
        efi_payload[UW_MEMORY_OVERCLOCKING_SWITCH] = 0;
        efi_payload[UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH] = 0;
        efi_read_size = UW_CPU_PERFORMANCE_AND_OVERCLOCKING_SWITCH + 1;
        uw_show_hidden_bios_options();
        assert(efi_reads == 2 && efi_writes == 1 && efi_allocations == 1 && efi_frees == 1);
    } else return 2;
    assert(!uniwill_ec_transaction_lock.held);
    return 0;
}
