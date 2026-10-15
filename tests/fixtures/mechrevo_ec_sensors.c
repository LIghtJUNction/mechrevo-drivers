/* Kernel API doubles; the complete production translation unit follows. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef unsigned short umode_t;
#define __init
#define __exit
#define module_init(x)
#define module_exit(x)
#define MODULE_DESCRIPTION(x)
#define MODULE_LICENSE(x)
#define MODULE_DEVICE_TABLE(t, x)
#define DMI_SYS_VENDOR 0
#define DMI_PRODUCT_NAME 1
#define DMI_BIOS_VERSION 2
struct dmi_system_id { int matches[4]; };
#define DMI_EXACT_MATCH(a,b) (a + 1)
#define PLATFORM_DEVID_NONE (-1)
#define UNIWILL_INTERFACE_WMI_STRID "uniwill_wmi"
#define ERR_PTR(e) ((void *)(intptr_t)(e))
#define IS_ERR(p) ((uintptr_t)(p) >= (uintptr_t)-4095)
#define PTR_ERR(p) ((intptr_t)(p))
enum hwmon_sensor_types { hwmon_temp, hwmon_fan };
enum { hwmon_temp_input, hwmon_temp_label, hwmon_temp_max };
#define HWMON_T_INPUT 1
#define HWMON_T_LABEL 2
struct device { int unused; };
struct platform_device { struct device dev; };
struct hwmon_ops {
    umode_t (*is_visible)(const void *, enum hwmon_sensor_types, u32, int);
    int (*read)(struct device *, enum hwmon_sensor_types, u32, int, long *);
    int (*read_string)(struct device *, enum hwmon_sensor_types, u32, int, const char **);
    void *write;
};
struct hwmon_channel_info { int type; const u32 *config; };
#define HWMON_CHANNEL_INFO(stype, ...) \
    (&(const struct hwmon_channel_info){ .type = hwmon_##stype, \
     .config = (const u32[]){__VA_ARGS__, 0} })
struct hwmon_chip_info {
    const struct hwmon_ops *ops;
    const struct hwmon_channel_info *const *info;
};
static const char *identity[] = { "MECHREVO", "Kuangshi16Pro Series GM6PX0X", "N.1.09MRO07" };
static const char *dmi_values[3];
static char *active_id;
static int active_ret, active_calls, ec_calls, ec_ret[2], register_ret[2];
static int load_ret, load_calls, after_load_ret;
static int platform_registers, platform_unregisters, hwmon_registers, hwmon_unregisters;
static u8 temps[2];
static struct platform_device mock_platform;
static struct device mock_hwmon;
static char events[16];
static unsigned event_count;
static bool dmi_match(int field, const char *value)
{ return dmi_values[field] && strcmp(dmi_values[field], value) == 0; }
static int uniwill_get_active_interface_id(char **id)
{ active_calls++; if (!active_ret) *id = active_id; return active_ret; }
static int request_module(const char *name)
{
    assert(strcmp(name, "uniwill_wmi") == 0);
    load_calls++;
    if (!load_ret) active_ret = after_load_ret;
    return load_ret;
}
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
    assert(address == 0x043e || address == 0x044f);
    int channel = address == 0x043e ? 0 : 1;
    ec_calls++;
    if (ec_ret[channel]) return ec_ret[channel];
    *value = temps[channel];
    return 0;
}
static struct platform_device *platform_device_register_simple(
    const char *name, int id, const void *resources, unsigned count)
{
    assert(strcmp(name, "mechrevo_ec_sensors") == 0);
    assert(id == -1 && resources == NULL && count == 0);
    platform_registers++; events[event_count++] = 'P';
    return register_ret[0] ? ERR_PTR(register_ret[0]) : &mock_platform;
}
static void platform_device_unregister(struct platform_device *device)
{
    assert(device == &mock_platform);
    platform_unregisters++; events[event_count++] = 'p';
}
static struct device *hwmon_device_register_with_info(struct device *parent,
    const char *name, void *data, const struct hwmon_chip_info *chip, const void *groups)
{
    assert(parent == &mock_platform.dev && data == NULL && groups == NULL);
    assert(strcmp(name, "mechrevo_ec") == 0);
    assert(chip->info[0]->type == hwmon_temp && chip->info[1] == NULL);
    assert(chip->info[0]->config[0] == 3 && chip->info[0]->config[1] == 3);
    assert(chip->info[0]->config[2] == 0 && chip->ops->write == NULL);
    hwmon_registers++; events[event_count++] = 'H';
    return register_ret[1] ? ERR_PTR(register_ret[1]) : &mock_hwmon;
}
static void hwmon_device_unregister(struct device *device)
{
    assert(device == &mock_hwmon);
    hwmon_unregisters++; events[event_count++] = 'h';
}
/* @DRIVER@ */
static void reset(void)
{
    for (int i = 0; i < 3; i++) dmi_values[i] = identity[i];
    active_id = "uniwill_wmi"; active_ret = 0; active_calls = 0; ec_calls = 0;
    load_ret = load_calls = after_load_ret = 0;
    ec_ret[0] = ec_ret[1] = register_ret[0] = register_ret[1] = 0;
    temps[0] = 63; temps[1] = 55;
    platform_registers = platform_unregisters = hwmon_registers = hwmon_unregisters = 0;
    event_count = 0; memset(events, 0, sizeof(events));
    mechrevo_sensors_device = NULL; mechrevo_sensors_hwmon = NULL;
}
static void identity_case(void)
{
    char suffixed[100];
    for (int field = 0; field < 3; field++) {
        for (int variant = 0; variant < 3; variant++) {
            reset(); snprintf(suffixed, sizeof(suffixed), "%s extra", identity[field]);
            dmi_values[field] = variant == 0 ? NULL : variant == 1 ? "other" : suffixed;
            assert(mechrevo_sensors_init() == -ENODEV);
            assert(active_calls == 0 && load_calls == 0 && ec_calls == 0 && platform_registers == 0);
        }
    }
}
static void transport_case(void)
{
    int errors[] = {-ENODEV, -EIO, -ETIMEDOUT, 1};
    for (unsigned i = 0; i < sizeof(errors)/sizeof(errors[0]); i++) {
        reset(); active_ret = after_load_ret = errors[i];
        assert(mechrevo_sensors_init() == (errors[i] < 0 ? errors[i] : -EIO));
        assert(ec_calls == 0 && platform_registers == 0);
        for (int channel = 0; channel < 2; channel++) {
            reset(); ec_ret[channel] = errors[i]; long value = 777;
            assert(mechrevo_sensors_temperature(channel, &value) == (errors[i] < 0 ? errors[i] : -EIO));
            assert(value == 777);
            ec_calls = 0;
            assert(mechrevo_sensors_init() == (errors[i] < 0 ? errors[i] : -EIO));
            assert(ec_calls == channel + 1 && platform_registers == 0);
        }
    }
    reset(); active_id = NULL; assert(mechrevo_sensors_init() == -ENODEV);
    assert(ec_calls == 0);
    reset(); active_id = "other"; assert(mechrevo_sensors_init() == -ENODEV);
    assert(ec_calls == 0);
}
static void loading_case(void)
{
    reset();
    assert(mechrevo_sensors_init() == 0 && load_calls == 0 && active_calls == 1);
    mechrevo_sensors_exit();
    assert(load_calls == 0);
    reset(); active_ret = -ENODEV;
    assert(mechrevo_sensors_init() == 0 && load_calls == 1 && active_calls == 2);
    assert(ec_calls == 2);
    mechrevo_sensors_exit();
    assert(load_calls == 1); /* Exit never touches the transport lifecycle. */
    int errors[] = {-ENOENT, -EPERM, -ETIMEDOUT, 1};
    for (unsigned i = 0; i < sizeof(errors)/sizeof(errors[0]); i++) {
        reset(); active_ret = -ENODEV; load_ret = errors[i];
        assert(mechrevo_sensors_init() == (errors[i] < 0 ? errors[i] : -EIO));
        assert(load_calls == 1 && active_calls == 1 && ec_calls == 0 && platform_registers == 0);
    }
    for (unsigned i = 0; i < sizeof(errors)/sizeof(errors[0]); i++) {
        reset(); active_ret = -ENODEV; after_load_ret = errors[i];
        assert(mechrevo_sensors_init() == (errors[i] < 0 ? errors[i] : -EIO));
        assert(load_calls == 1 && active_calls == 2 && ec_calls == 0 && platform_registers == 0);
    }
    reset(); active_ret = -ENODEV; active_id = "other";
    assert(mechrevo_sensors_init() == -ENODEV && load_calls == 1 && ec_calls == 0);
    reset(); active_id = "other";
    assert(mechrevo_sensors_init() == -ENODEV && load_calls == 0 && ec_calls == 0);
    reset(); active_ret = -EIO;
    assert(mechrevo_sensors_init() == -EIO && load_calls == 0 && ec_calls == 0);
}
static void temperature_case(void)
{
    reset();
    for (int channel = 0; channel < 2; channel++) {
        for (int raw = 0; raw <= 255; raw++) {
            temps[channel] = raw; long value = -987;
            int ret = mechrevo_sensors_read(NULL, hwmon_temp, hwmon_temp_input, channel, &value);
            assert(ret == (raw <= 125 ? 0 : -ENODATA));
            assert(value == (raw <= 125 ? raw * 1000 : -987));
            if (raw > 125) {
                assert(mechrevo_sensors_init() == -ENODATA);
                assert(platform_registers == 0);
            }
        }
        temps[channel] = 63;
    }
}
static void interface_case(void)
{
    reset();
    for (int type = hwmon_temp; type <= hwmon_fan; type++)
        for (int attr = 0; attr < 4; attr++)
            for (int channel = -1; channel < 4; channel++) {
                bool valid = type == hwmon_temp && channel >= 0 && channel < 2;
                long value = 123; const char *label = "unchanged";
                assert(mechrevo_sensors_is_visible(NULL, type, attr, channel) ==
                       (valid && (attr == hwmon_temp_input || attr == hwmon_temp_label) ? 0444 : 0));
                int before = ec_calls;
                assert(mechrevo_sensors_read(NULL, type, attr, channel, &value) ==
                       (valid && attr == hwmon_temp_input ? 0 : -EOPNOTSUPP));
                assert(ec_calls == before + (valid && attr == hwmon_temp_input));
                if (!(valid && attr == hwmon_temp_input)) assert(value == 123);
                assert(mechrevo_sensors_read_string(NULL, type, attr, channel, &label) ==
                       (valid && attr == hwmon_temp_label ? 0 : -EOPNOTSUPP));
                assert(strcmp(label, valid && attr == hwmon_temp_label ?
                       (channel == 0 ? "CPU EC" : "GPU EC") : "unchanged") == 0);
            }
}
static void lifecycle_case(void)
{
    reset(); register_ret[0] = -ENOMEM;
    assert(mechrevo_sensors_init() == -ENOMEM);
    assert(platform_registers == 1 && !platform_unregisters && !hwmon_registers);
    assert(mechrevo_sensors_device == NULL && mechrevo_sensors_hwmon == NULL);
    reset(); register_ret[1] = -EIO;
    assert(mechrevo_sensors_init() == -EIO);
    assert(platform_registers == 1 && platform_unregisters == 1 && hwmon_registers == 1 && !hwmon_unregisters);
    assert(strcmp(events, "PHp") == 0);
    assert(mechrevo_sensors_device == NULL && mechrevo_sensors_hwmon == NULL);
    register_ret[1] = 0;
    assert(mechrevo_sensors_init() == 0); /* Retry after failure. */
    mechrevo_sensors_exit();
    assert(strcmp(events, "PHpPHhp") == 0);
    reset();
    assert(mechrevo_sensors_init() == 0 && ec_calls == 2);
    assert(platform_registers == 1 && hwmon_registers == 1);
    mechrevo_sensors_exit();
    assert(strcmp(events, "PHhp") == 0);
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    if (!strcmp(argv[1], "identity")) identity_case();
    else if (!strcmp(argv[1], "transport")) transport_case();
    else if (!strcmp(argv[1], "temperature")) temperature_case();
    else if (!strcmp(argv[1], "interface")) interface_case();
    else if (!strcmp(argv[1], "lifecycle")) lifecycle_case();
    else if (!strcmp(argv[1], "loading")) loading_case();
    else assert(false);
    return 0;
}
