/* Compile the actual feature detection and ioctl helpers with transport spies. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#define KERNEL_VERSION(a,b,c) (((a) << 16) | ((b) << 8) | (c))
#define DMI_SYS_VENDOR 0
#define DMI_BOARD_NAME 1
#define DMI_PRODUCT_SKU 2
#define pr_debug(...) ((void)0)
/* @TYPES@ */
static struct uniwill_device_features_t uniwill_device_features;
static struct uniwill_device_features_t *uw_feats;
static bool uw_feats_loaded;
static const char *dmi[3];
static int reads, writes, read_error, write_error;
static bool double_pl4;
static u8 ec[65536], written_index, written_value;
static bool dmi_match(int field, const char *value)
{ return dmi[field] && !strcmp(dmi[field], value); }
static int uniwill_read_ec_ram(u16 addr, u8 *value)
{ reads++; if (read_error < 0) return read_error; *value = ec[addr]; return 0; }
static int uw_update_mode(u8 clear, u8 set)
{ writes++; if (write_error) return write_error; ec[0x0751] = (ec[0x0751] & ~clear) | set; return 0; }
static int uw_write_tdp(u8 index, u8 value)
{ writes++; written_index = index; written_value = value; return write_error; }
static int has_double_pl4(bool *value)
{ *value = double_pl4; return 0; }
static int unavailable(bool *value) { *value = false; return 0; }
#define uw_has_charging_priority unavailable
#define uw_has_charging_profile unavailable
#define uw_has_ac_auto_boot unavailable
#define uw_has_usb_powershare unavailable
#define uw_has_mini_led_local_dimming unavailable
#define uw_has_hidden_bios_options unavailable
static int has_universal_ec_fan_control(void) { return 0; }
/* @TABLES@ */
/* @PRODUCTION@ */
static void detect(const char *vendor, const char *board, const char *sku)
{
    memset(&uniwill_device_features, 0, sizeof(uniwill_device_features));
    uw_feats_loaded = false;
    dmi[0] = vendor; dmi[1] = board; dmi[2] = sku;
    uw_feats = uniwill_get_device_features();
    uw_id_tdp();
    assert(writes == 0); /* Identification must never apply a power target. */
}
static void reset(void)
{
    reads = writes = read_error = write_error = 0;
    double_pl4 = false;
    memset(ec, 0, sizeof(ec));
}
static void gx4hrxl(void)
{
    reset(); detect("MECHREVO", "GX4HRXL", NULL);
    assert(uw_get_performance_profile_count() == 3);
    assert(!uw_feats->uniwill_profile_v1); /* No implicit legacy fan-curve copy. */
    assert(!uw_feats->uniwill_custom_profile_mode_needed);
    for (int initial = 0; initial <= 255; initial++) {
        for (int profile = 1; profile <= 3; profile++) {
            ec[0x0751] = initial;
            assert(uw_set_performance_profile_v1(profile) == 0);
            int bits[] = {0, 0xa0, 0, 0x10};
            assert(ec[0x0751] == ((initial & ~0xb0) | bits[profile]));
        }
    }
    int before = writes;
    assert(uw_set_performance_profile_v1(0) == -EINVAL);
    assert(uw_set_performance_profile_v1(4) == -EINVAL);
    for (int index = 0; index < 3; index++) {
        assert(uw_get_tdp(index) == -ENODEV);
        assert(uw_set_tdp(index, 50) == -ENODEV);
    }
    assert(writes == before);
    write_error = -ETIMEDOUT;
    assert(uw_set_performance_profile_v1(PROFILE_ENTHUSIAST) == -ETIMEDOUT);
}
static void jiaolong(void)
{
    reset(); detect("MECHREVO", "JIAOLONG Series-X6xR55xK", NULL);
    assert(uw_get_performance_profile_count() == 0); /* Custom mode is not classic mode. */
#if LINUX_VERSION_CODE >= KERNEL_VERSION(4, 18, 0)
    assert(uw_feats->uniwill_custom_profile_mode_needed);
    int maximum[] = {162, 162, 195};
    for (int doubled = 0; doubled < 2; doubled++) {
        uw_feats->uniwill_has_double_pl4 = doubled;
        for (int index = 0; index < 3; index++) {
            assert(uw_get_tdp_min(index) == 25 && uw_get_tdp_max(index) == maximum[index]);
            for (int value = 0; value <= maximum[index] + 1; value++) {
                int before = writes;
                bool valid = value >= 25 && value <= maximum[index];
                assert(uw_set_tdp(index, value) == (valid ? 0 : -EINVAL));
                assert(writes == before + valid);
                if (valid) {
                    assert(written_index == index);
                    assert(written_value == (doubled && index == 2 ? value / 2 : value));
                }
            }
            ec[0x0783 + index] = 51;
            assert(uw_get_tdp(index) == (doubled && index == 2 ? 102 : 51));
        }
    }
    write_error = -EIO;
    assert(uw_set_tdp(0, 50) == -EIO);
    read_error = -ETIMEDOUT;
    assert(uw_get_tdp(0) == -ETIMEDOUT);
#else
    assert(!uw_feats->uniwill_custom_profile_mode_needed);
    assert(uw_set_tdp(0, 50) == -ENODEV);
#endif
    assert(uw_set_tdp(3, 50) == -EINVAL);
    assert(uw_get_tdp(255) == -EINVAL);
}
static void identity(void)
{
    const char *vendors[] = {NULL, "Other", "Other MECHREVO", "MECHREVO extra"};
    const char *boards[] = {"GX4HRXL", "JIAOLONG Series-X6xR55xK"};
    for (unsigned b = 0; b < 2; b++) {
        for (unsigned v = 0; v < 4; v++) {
            reset(); detect(vendors[v], boards[b], NULL);
            assert(!uw_feats->uniwill_profile_v1 && !uw_feats->uniwill_custom_profile_mode_needed);
            assert(uw_set_tdp(0, 50) == -ENODEV);
            assert(uw_set_performance_profile_v1(PROFILE_POWERSAVE) == -ENODEV);
            assert(writes == 0);
        }
    }
    const char *unknown[] = {NULL, "GX4HRXL extra", "GX4HRX", "X6FR57TK",
        "JIAOLONG Series-X6xR55xK extra", "GM6BG0Q", "GM6PX0X"};
    for (unsigned b = 0; b < sizeof(unknown)/sizeof(*unknown); b++) {
        reset(); detect("MECHREVO", unknown[b], NULL);
        assert(!uw_feats->uniwill_profile_v1 && !uw_feats->uniwill_custom_profile_mode_needed);
        assert(uw_set_tdp(0, 50) == -ENODEV && writes == 0);
    }
}
static void existing(void)
{
    reset(); detect("TUXEDO", "GXxHRXx", NULL);
    assert(uw_feats->uniwill_profile_v1 && uw_get_performance_profile_count() == 3);
#if LINUX_VERSION_CODE >= KERNEL_VERSION(4, 18, 0)
    assert(uw_feats->uniwill_custom_profile_mode_needed);
    reset(); detect("TUXEDO", "X6FR5xxY", "STELLARIS16A07");
    assert(uw_get_tdp_min(0) == 25 && uw_get_tdp_max(0) == 162);
#endif
}
static void retry(void)
{
    reset(); read_error = -EIO;
    detect("MECHREVO", "GX4HRXL", NULL);
    assert(!uw_feats_loaded);
    read_error = 0;
    assert(uniwill_get_device_features() == uw_feats && uw_feats_loaded);
    assert(reads == 2 && writes == 0);
    assert(uniwill_get_device_features() == uw_feats && reads == 2);
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    if (!strcmp(argv[1], "gx4hrxl")) gx4hrxl();
    else if (!strcmp(argv[1], "jiaolong")) jiaolong();
    else if (!strcmp(argv[1], "identity")) identity();
    else if (!strcmp(argv[1], "existing")) existing();
    else if (!strcmp(argv[1], "retry")) retry();
    else assert(false);
    return 0;
}
