/* Userspace mocks for the exact driver functions injected by test_uniwill_runtime.py.
 * No device, sysfs, EC, module loading or root privileges are used. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#define __user

struct uniwill_device_features_t {
	bool uniwill_has_mini_led_local_dimming;
	bool uniwill_has_double_pl4;
};
static struct uniwill_device_features_t uniwill_device_features;
#define features uniwill_device_features
static struct uniwill_device_features_t *uw_feats = &uniwill_device_features;

struct platform_device { struct { int kobj; } dev; };
static int uw_mini_led_local_dimming_attr_group;
static bool uw_mini_led_local_dimming_loaded;
static int group_status, group_calls, dimming_calls;
static int sysfs_create_group(const int *kobj, const void *group)
{
	(void)kobj;
	(void)group;
	++group_calls;
	return group_status;
}
static int uw_set_mini_led_local_dimming(bool enabled)
{
	assert(!enabled);
	++dimming_calls;
	return 0;
}

static int minimums[] = {5, 6, 7};
static int maximums[] = {80, 90, 200};
static int *tdp_min_defs = minimums;
static int *tdp_max_defs = maximums;
static int ec_status, ec_reads, copy_fault, copies;
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
	assert(address >= 0x0783 && address <= 0x0785);
	++ec_reads;
	if (ec_status)
		return ec_status;
	*value = 64;
	return 0;
}
static unsigned long copy_to_user(void *destination, const void *source, size_t length)
{
	assert(length == sizeof(u32));
	if (copy_fault)
		return length;
	memcpy(destination, source, length);
	++copies;
	return 0;
}

enum {
	R_UW_TDP0, R_UW_TDP1, R_UW_TDP2,
	R_UW_TDP0_MIN, R_UW_TDP1_MIN, R_UW_TDP2_MIN,
	R_UW_TDP0_MAX, R_UW_TDP1_MAX, R_UW_TDP2_MAX
};

/* @DRIVER_FUNCTIONS@ */

static long dispatch(unsigned int cmd, unsigned long arg)
{
	switch (cmd) {
/* @DRIVER_TDP_CASES@ */
	default:
		return -ENOTTY;
	}
}

/* Keep the driver's outer ioctl entry intact so signed errno propagation is tested. */
struct file { int unused; };
static struct { const char *version; } mock_module = { "test" };
#define THIS_MODULE (&mock_module)
enum { R_MOD_VERSION = 100, R_HWCHECK_CL, R_HWCHECK_UW };
static u32 id_check_clevo, id_check_uniwill;
static u32 clevo_identify(void) { return 0; }
static u32 uniwill_identify(void) { return 1; }
static long clevo_ioctl_interface(struct file *file, unsigned int cmd, unsigned long arg)
{
	(void)file;
	(void)cmd;
	(void)arg;
	return 0;
}
static long uniwill_ioctl_interface(struct file *file, unsigned int cmd, unsigned long arg)
{
	(void)file;
	return dispatch(cmd, arg);
}

/* The unchanged entry also contains pre-existing unchecked copy results. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-but-set-variable"
/* @DRIVER_IOCTL_ENTRY@ */
#pragma GCC diagnostic pop

int main(int argc, char **argv)
{
	assert(argc == 2);
	const char *test = argv[1];
	u32 value = 0xdeadbeef;
	struct platform_device device = {0};

	if (strcmp(test, "miniled_unsupported") == 0) {
		uw_mini_led_local_dimming_init(&device);
		assert(!uw_mini_led_local_dimming_loaded);
		assert(group_calls == 0 && dimming_calls == 0);
	} else if (strcmp(test, "miniled_supported") == 0) {
		features.uniwill_has_mini_led_local_dimming = true;
		uw_mini_led_local_dimming_init(&device);
		assert(uw_mini_led_local_dimming_loaded);
		assert(group_calls == 1 && dimming_calls == 1);
	} else if (strcmp(test, "miniled_registration_failure") == 0) {
		features.uniwill_has_mini_led_local_dimming = true;
		group_status = -ENOMEM;
		uw_mini_led_local_dimming_init(&device);
		assert(!uw_mini_led_local_dimming_loaded);
		assert(group_calls == 1 && dimming_calls == 0);
	} else if (strcmp(test, "tdp_unsupported") == 0) {
		tdp_min_defs = NULL;
		tdp_max_defs = NULL;
		for (unsigned int cmd = R_UW_TDP0; cmd <= R_UW_TDP2_MAX; ++cmd)
			assert(fop_ioctl(NULL, cmd, (unsigned long)&value) == -ENODEV);
		assert(value == 0xdeadbeef && copies == 0 && ec_reads == 0);
	} else if (strcmp(test, "tdp_supported") == 0) {
		u32 expected[] = {64, 64, 64, 5, 6, 7, 80, 90, 200};
		for (unsigned int cmd = R_UW_TDP0; cmd <= R_UW_TDP2_MAX; ++cmd) {
			assert(fop_ioctl(NULL, cmd, (unsigned long)&value) == 0);
			assert(value == expected[cmd]);
		}
		assert(copies == 9 && ec_reads == 3);
	} else if (strcmp(test, "tdp_ec_error") == 0) {
		ec_status = -EIO;
		for (unsigned int cmd = R_UW_TDP0; cmd <= R_UW_TDP2; ++cmd)
			assert(fop_ioctl(NULL, cmd, (unsigned long)&value) == -EIO);
		assert(value == 0xdeadbeef && copies == 0 && ec_reads == 3);
	} else if (strcmp(test, "tdp_invalid_index") == 0) {
		assert(uw_get_tdp(3) == -EINVAL);
		assert(uw_get_tdp_min(3) == -EINVAL);
		assert(uw_get_tdp_max(3) == -EINVAL);
		assert(ec_reads == 0);
	} else if (strcmp(test, "tdp_disabled_limit") == 0) {
		minimums[0] = 0;
		maximums[0] = 0;
		assert(fop_ioctl(NULL, R_UW_TDP0, (unsigned long)&value) == -ENODEV);
		assert(fop_ioctl(NULL, R_UW_TDP0_MIN, (unsigned long)&value) == -ENODEV);
		assert(fop_ioctl(NULL, R_UW_TDP0_MAX, (unsigned long)&value) == -ENODEV);
		assert(value == 0xdeadbeef && copies == 0 && ec_reads == 0);
	} else if (strcmp(test, "tdp_copy_fault") == 0) {
		copy_fault = 1;
		for (unsigned int cmd = R_UW_TDP0; cmd <= R_UW_TDP2_MAX; ++cmd)
			assert(fop_ioctl(NULL, cmd, (unsigned long)&value) == -EFAULT);
		assert(value == 0xdeadbeef && copies == 0);
	} else if (strcmp(test, "tdp_double_pl4") == 0) {
		features.uniwill_has_double_pl4 = true;
		assert(fop_ioctl(NULL, R_UW_TDP2, (unsigned long)&value) == 0);
		assert(value == 128 && copies == 1 && ec_reads == 1);
	} else {
		return 2;
	}
	return 0;
}
