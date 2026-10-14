/* Userspace mocks for the exact driver functions injected by test_uniwill_runtime.py.
 * No device, sysfs, EC, module loading or root privileges are used. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include "tuxedo_io_ioctl.h"

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#define __user

struct uniwill_device_features_t {
	u32 model;
	bool uniwill_has_double_pl4;
	bool uniwill_has_universal_ec_fan_control;
	bool uniwill_custom_profile_mode_needed;
	bool uniwill_profile_v1_two_profs;
	bool uniwill_profile_v1_three_profs;
	bool uniwill_profile_v1_three_profs_leds_only;
};
static struct uniwill_device_features_t uniwill_device_features;
#define features uniwill_device_features
static struct uniwill_device_features_t *uw_feats = &uniwill_device_features;

static int minimums[] = {5, 6, 7};
static int maximums[] = {80, 90, 200};
static int *tdp_min_defs = minimums;
static int *tdp_max_defs = maximums;
static int ec_status, ec_reads, ec_write_status, ec_writes;
static u16 ec_error_address, ec_write_error_address;
static u16 written_addresses[8];
static u8 written_values[8], ec_value = 64;
static int copy_fault, copies, copy_attempts, copy_from_fault, input_copies;
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
	++ec_reads;
	if (ec_status && (!ec_error_address || ec_error_address == address))
		return ec_status;
	*value = ec_value;
	return 0;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
	assert(ec_writes < 8);
	written_addresses[ec_writes] = address;
	written_values[ec_writes++] = value;
	if (ec_write_status && (!ec_write_error_address || ec_write_error_address == address))
		return ec_write_status;
	return 0;
}
static unsigned long copy_to_user(void *destination, const void *source, size_t length)
{
	++copy_attempts;
	if (copy_fault)
		return 1;
	memcpy(destination, source, length);
	++copies;
	return 0;
}
static unsigned long copy_from_user(void *destination, const void *source, size_t length)
{
	++input_copies;
	/* A partial copy must also stop the driver before it uses the argument. */
	memcpy(destination, source, copy_from_fault ? length / 2 : length);
	return copy_from_fault ? 1 : 0;
}
static int fan_status, fan_calls, fan_auto_calls;
static u32 selected_fan;
static u8 selected_speed;
static u32 uw_set_fan(u32 fan_index, u8 fan_speed)
{
	++fan_calls;
	selected_fan = fan_index;
	selected_speed = fan_speed;
	return fan_status;
}
static u32 uw_set_fan_auto(void)
{
	++fan_auto_calls;
	return fan_status;
}
static int interface_status;
static int uniwill_get_active_interface_id(char **name)
{
	if (name)
		*name = "mock-uniwill";
	return interface_status;
}

/* @DRIVER_INTERFACE_TYPES@ */
/* @DRIVER_PROFILE_PROTOTYPE@ */

/* @DRIVER_FUNCTIONS@ */

/* Keep the driver's outer ioctl entry intact so signed errno propagation is tested. */
struct file { int unused; };
static struct { const char *version; } mock_module = { "test" };
#define THIS_MODULE (&mock_module)
static u32 id_check_clevo, id_check_uniwill;
static u32 clevo_identify(void) { return 0; }
static u32 uniwill_identify(void) { return 1; }
static int clevo_calls;
static long clevo_ioctl_interface(struct file *file, unsigned int cmd, unsigned long arg)
{
	(void)file;
	(void)cmd;
	(void)arg;
	++clevo_calls;
	return 0;
}

/* The kernel does not enable these userspace fixture warnings. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-but-set-variable"
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_UNIWILL_IOCTL@ */
/* @DRIVER_IOCTL_ENTRY@ */
#pragma GCC diagnostic pop

static const unsigned int tdp_reads[] = {
	R_UW_TDP0, R_UW_TDP1, R_UW_TDP2,
	R_UW_TDP0_MIN, R_UW_TDP1_MIN, R_UW_TDP2_MIN,
	R_UW_TDP0_MAX, R_UW_TDP1_MAX, R_UW_TDP2_MAX
};
static const unsigned int ec_reads_cmds[] = {
	R_UW_FANSPEED, R_UW_FANSPEED2, R_UW_FAN_TEMP,
	R_UW_FAN_TEMP2, R_UW_MODE, R_UW_MODE_ENABLE
};
static const unsigned int writes_cmds[] = {
	W_UW_FANSPEED, W_UW_FANSPEED2, W_UW_MODE,
	W_UW_TDP0, W_UW_TDP1, W_UW_TDP2, W_UW_PERF_PROF
};
#define COUNT(array) (sizeof(array) / sizeof((array)[0]))
#define IOCTL(command, value) fop_ioctl(NULL, command, (unsigned long)&(value))

int main(int argc, char **argv)
{
	assert(argc == 2);
	const char *test = argv[1];
	u32 value = 0xdeadbeef;

	if (strcmp(test, "tdp_unsupported") == 0) {
		tdp_min_defs = NULL;
		tdp_max_defs = NULL;
		for (size_t i = 0; i < COUNT(tdp_reads); ++i)
			assert(IOCTL(tdp_reads[i], value) == -ENODEV);
		assert(value == 0xdeadbeef && copies == 0 && ec_reads == 0);
	} else if (strcmp(test, "tdp_supported") == 0) {
		u32 expected[] = {64, 64, 64, 5, 6, 7, 80, 90, 200};
		for (size_t i = 0; i < COUNT(tdp_reads); ++i) {
			assert(IOCTL(tdp_reads[i], value) == 0);
			assert(value == expected[i]);
		}
		assert(copies == 9 && ec_reads == 3);
	} else if (strcmp(test, "tdp_ec_error") == 0) {
		ec_status = -EIO;
		for (size_t i = 0; i < 3; ++i)
			assert(IOCTL(tdp_reads[i], value) == -EIO);
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
		for (size_t i = 0; i < COUNT(tdp_reads); ++i)
			assert(IOCTL(tdp_reads[i], value) == -EFAULT);
		assert(value == 0xdeadbeef && copies == 0);
	} else if (strcmp(test, "tdp_double_pl4") == 0) {
		features.uniwill_has_double_pl4 = true;
		assert(fop_ioctl(NULL, R_UW_TDP2, (unsigned long)&value) == 0);
		assert(value == 128 && copies == 1 && ec_reads == 1);
	} else if (strcmp(test, "ec_read_error") == 0) {
		ec_status = -EIO;
		for (size_t i = 0; i < COUNT(ec_reads_cmds); ++i)
			assert(IOCTL(ec_reads_cmds[i], value) == -EIO);
		assert(value == 0xdeadbeef && copy_attempts == 0 && ec_reads == 6);
	} else if (strcmp(test, "ec_read_success") == 0) {
		for (size_t i = 0; i < COUNT(ec_reads_cmds); ++i) {
			assert(IOCTL(ec_reads_cmds[i], value) == 0);
			assert(value == 64);
		}
		assert(copies == 6 && ec_reads == 6);
	} else if (strcmp(test, "fan_zero_mapping") == 0) {
		ec_value = 1;
		assert(IOCTL(R_UW_FANSPEED, value) == 0 && value == 1);
		features.uniwill_has_universal_ec_fan_control = true;
		assert(IOCTL(R_UW_FANSPEED, value) == 0 && value == 0);
		assert(IOCTL(R_UW_FANSPEED2, value) == 0 && value == 0);
	} else if (strcmp(test, "all_read_copy_faults") == 0) {
		const unsigned int metadata[] = {
			R_UW_HW_IF_STR, R_UW_MODEL_ID, R_UW_FANS_OFF_AVAILABLE,
			R_UW_FANS_MIN_SPEED, R_UW_PROFS_AVAILABLE,
			R_MOD_VERSION, R_HWCHECK_CL, R_HWCHECK_UW
		};
		copy_fault = 1;
		for (size_t i = 0; i < COUNT(ec_reads_cmds); ++i)
			assert(IOCTL(ec_reads_cmds[i], value) == -EFAULT);
		for (size_t i = 0; i < COUNT(metadata); ++i)
			assert(IOCTL(metadata[i], value) == -EFAULT);
		interface_status = -ENODEV;
		assert(IOCTL(R_UW_HW_IF_STR, value) == -EFAULT);
		assert(value == 0xdeadbeef && copies == 0 && copy_attempts == 15);
		/* Generic output faults return before invoking either hardware dispatch. */
		assert(clevo_calls == 12);
	} else if (strcmp(test, "read_metadata_success") == 0) {
		char text[32];
		assert(IOCTL(R_MOD_VERSION, text) == 0 && strcmp(text, "test") == 0);
		assert(IOCTL(R_HWCHECK_CL, value) == 0 && value == 0);
		assert(IOCTL(R_HWCHECK_UW, value) == 0 && value == 1);
		assert(IOCTL(R_UW_HW_IF_STR, text) == 0 && strcmp(text, "mock-uniwill") == 0);
		interface_status = -ENODEV;
		assert(IOCTL(R_UW_HW_IF_STR, text) == 0 && text[0] == '\0');
		features.model = 42;
		assert(IOCTL(R_UW_MODEL_ID, value) == 0 && value == 42);
		assert(IOCTL(R_UW_FANS_OFF_AVAILABLE, value) == 0 && value == 1);
		assert(IOCTL(R_UW_FANS_MIN_SPEED, value) == 0 && value == 25);
		assert(IOCTL(R_UW_PROFS_AVAILABLE, value) == 0 && value == 0);
		features.uniwill_profile_v1_two_profs = true;
		assert(IOCTL(R_UW_PROFS_AVAILABLE, value) == 0 && value == 2);
		features.uniwill_profile_v1_two_profs = false;
		features.uniwill_profile_v1_three_profs = true;
		assert(IOCTL(R_UW_PROFS_AVAILABLE, value) == 0 && value == 3);
		features.uniwill_profile_v1_three_profs = false;
		features.uniwill_profile_v1_three_profs_leds_only = true;
		assert(IOCTL(R_UW_PROFS_AVAILABLE, value) == 0 && value == 3);
	} else if (strcmp(test, "write_copy_faults") == 0) {
		copy_from_fault = 1;
		features.uniwill_custom_profile_mode_needed = true;
		for (size_t i = 0; i < COUNT(writes_cmds); ++i)
			assert(IOCTL(writes_cmds[i], value) == -EFAULT);
		assert(input_copies == 7 && ec_reads == 0 && ec_writes == 0);
		assert(fan_calls == 0 && fan_auto_calls == 0);
	} else if (strcmp(test, "fan_write_success") == 0) {
		for (value = 0; value <= UINT8_MAX; ++value) {
			assert(IOCTL(W_UW_FANSPEED, value) == 0);
			assert(selected_fan == 0 && selected_speed == value);
			assert(IOCTL(W_UW_FANSPEED2, value) == 0);
			assert(selected_fan == 1 && selected_speed == value);
		}
		assert(fan_calls == 512 && input_copies == 512);
		assert(IOCTL(W_UW_FANAUTO, value) == 0 && fan_auto_calls == 1);
	} else if (strcmp(test, "fan_write_out_of_range") == 0) {
		const u32 invalid_values[] = {256, UINT32_MAX};
		for (size_t i = 0; i < COUNT(invalid_values); ++i) {
			value = invalid_values[i];
			assert(IOCTL(W_UW_FANSPEED, value) == -EINVAL);
			assert(IOCTL(W_UW_FANSPEED2, value) == -EINVAL);
		}
		assert(input_copies == 4 && fan_calls == 0 && fan_auto_calls == 0);
		assert(ec_reads == 0 && ec_writes == 0);
	} else if (strcmp(test, "fan_write_error") == 0) {
		value = 52;
		fan_status = -EINVAL;
		assert(IOCTL(W_UW_FANSPEED, value) == -EINVAL);
		assert(IOCTL(W_UW_FANSPEED2, value) == -EINVAL);
		assert(IOCTL(W_UW_FANAUTO, value) == -EINVAL);
		assert(fan_calls == 2 && fan_auto_calls == 1);
	} else if (strcmp(test, "mode_write_success") == 0) {
		value = 0x1234;
		assert(IOCTL(W_UW_MODE, value) == 0);
		assert(ec_writes == 1 && written_addresses[0] == 0x0751 && written_values[0] == 0x34);
	} else if (strcmp(test, "mode_write_error") == 0) {
		ec_write_status = -EIO;
		assert(IOCTL(W_UW_MODE, value) == -EIO && ec_writes == 1);
	} else if (strcmp(test, "tdp_write_unsupported") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		tdp_min_defs = NULL;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == -ENODEV);
		assert(IOCTL(W_UW_TDP1, value) == -ENODEV);
		assert(IOCTL(W_UW_TDP2, value) == -ENODEV);
		assert(ec_reads == 0 && ec_writes == 0);
	} else if (strcmp(test, "tdp_write_invalid_index") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		assert(uw_set_tdp(3, 30) == -EINVAL);
		assert(ec_reads == 0 && ec_writes == 0);
	} else if (strcmp(test, "tdp_write_invalid_value") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		value = 4;
		assert(IOCTL(W_UW_TDP0, value) == -EINVAL);
		value = 81;
		assert(IOCTL(W_UW_TDP0, value) == -EINVAL);
		value = UINT32_MAX;
		assert(IOCTL(W_UW_TDP0, value) == -EINVAL);
		assert(ec_reads == 0 && ec_writes == 0);
	} else if (strcmp(test, "tdp_write_invalid_max") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		value = 30;
		tdp_max_defs = NULL;
		assert(IOCTL(W_UW_TDP0, value) == -ENODEV);
		tdp_max_defs = maximums;
		maximums[0] = 0;
		assert(IOCTL(W_UW_TDP0, value) == -ENODEV);
		assert(ec_reads == 0 && ec_writes == 0);
	} else if (strcmp(test, "tdp_write_success") == 0) {
		const unsigned int cmds[] = {W_UW_TDP0, W_UW_TDP1, W_UW_TDP2};
		value = 30;
		for (size_t i = 0; i < COUNT(cmds); ++i) {
			assert(IOCTL(cmds[i], value) == 0);
			assert(written_addresses[i] == 0x0783 + i && written_values[i] == 30);
		}
		assert(ec_reads == 0 && ec_writes == 3);
	} else if (strcmp(test, "tdp_write_double_pl4") == 0) {
		features.uniwill_has_double_pl4 = true;
		value = 129;
		assert(IOCTL(W_UW_TDP2, value) == 0);
		assert(ec_writes == 1 && written_addresses[0] == 0x0785 && written_values[0] == 64);
	} else if (strcmp(test, "tdp_write_ec_error") == 0) {
		ec_write_status = -EIO;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == -EIO);
		assert(IOCTL(W_UW_TDP1, value) == -EIO);
		assert(IOCTL(W_UW_TDP2, value) == -EIO);
		assert(ec_writes == 3);
	} else if (strcmp(test, "tdp_profile_read_error") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		ec_status = -EIO;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == -EIO);
		assert(ec_reads == 1 && ec_writes == 0);
	} else if (strcmp(test, "tdp_profile_write_error") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		ec_write_status = -EIO;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == -EIO);
		assert(ec_reads == 1 && ec_writes == 1 && written_addresses[0] == 0x0751);
	} else if (strcmp(test, "tdp_profile_success") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == 0);
		assert(ec_reads == 1 && ec_writes == 2);
		assert(written_addresses[0] == 0x0751 && written_values[0] == 0x50);
		assert(written_addresses[1] == 0x0783 && written_values[1] == 30);
	} else if (strcmp(test, "tdp_ec_error_after_profile") == 0) {
		features.uniwill_custom_profile_mode_needed = true;
		ec_write_status = -EIO;
		ec_write_error_address = 0x0783;
		value = 30;
		assert(IOCTL(W_UW_TDP0, value) == -EIO);
		assert(ec_writes == 2 && written_addresses[1] == 0x0783);
	} else if (strcmp(test, "profile_read_error") == 0) {
		ec_status = -EIO;
		value = PROFILE_OVERBOOST;
		assert(IOCTL(W_UW_PERF_PROF, value) == -EIO);
		assert(ec_reads == 1 && ec_writes == 0);
	} else if (strcmp(test, "profile_write_error") == 0) {
		ec_write_status = -EIO;
		value = PROFILE_OVERBOOST;
		assert(IOCTL(W_UW_PERF_PROF, value) == -EIO);
		assert(ec_reads == 1 && ec_writes == 1);
	} else if (strcmp(test, "profile_invalid") == 0) {
		value = 0;
		assert(IOCTL(W_UW_PERF_PROF, value) == -EINVAL);
		value = 4;
		assert(IOCTL(W_UW_PERF_PROF, value) == -EINVAL);
		assert(ec_writes == 0);
	} else if (strcmp(test, "profile_success") == 0) {
		u8 expected[] = {0xef, 0x4f, 0x5f};
		ec_value = 0xff;
		for (value = 1; value <= 3; ++value) {
			assert(IOCTL(W_UW_PERF_PROF, value) == 0);
			assert(written_addresses[value - 1] == 0x0751);
			assert(written_values[value - 1] == expected[value - 1]);
		}
	} else if (strcmp(test, "unchanged_noop_dispatch") == 0) {
		assert(IOCTL(W_UW_MODE_ENABLE, value) == 0);
		assert(IOCTL(0xffffffffU, value) == 0);
		assert(input_copies == 0 && ec_reads == 0 && ec_writes == 0);
#ifdef DEBUG
	} else if (strcmp(test, "debug_copy_from_faults") == 0) {
		u32 args[10] = {0x51, 0x07, 0x34};
		copy_from_fault = 1;
		assert(IOCTL(R_TF_BC, args) == -EFAULT);
		assert(IOCTL(W_TF_BC, args) == -EFAULT);
		assert(ec_reads == 0 && ec_writes == 0 && copy_attempts == 0);
	} else if (strcmp(test, "debug_read_error") == 0) {
		u32 args[10] = {0x51, 0x07, 0x34};
		ec_status = -EIO;
		assert(IOCTL(R_TF_BC, args) == -EIO);
		assert(args[0] == 0x51 && copy_attempts == 0);
	} else if (strcmp(test, "debug_write_error") == 0) {
		u32 args[10] = {0x51, 0x07, 0x34};
		ec_write_status = -EIO;
		assert(IOCTL(W_TF_BC, args) == -EIO);
		assert(args[0] == 0x51 && copy_attempts == 0);
	} else if (strcmp(test, "debug_copy_to_faults") == 0) {
		u32 args[10] = {0x51, 0x07, 0x34};
		copy_fault = 1;
		assert(IOCTL(R_TF_BC, args) == -EFAULT);
		assert(IOCTL(W_TF_BC, args) == -EFAULT);
		assert(copy_attempts == 2 && ec_reads == 1 && ec_writes == 1);
	} else if (strcmp(test, "debug_success") == 0) {
		u32 args[10] = {0x51, 0x07, 0x34};
		assert(IOCTL(R_TF_BC, args) == 0 && args[0] == 64);
		args[0] = 0x51;
		assert(IOCTL(W_TF_BC, args) == 0 && args[0] == 0);
		assert(ec_writes == 1 && written_addresses[0] == 0x0751 && written_values[0] == 0x34);
#endif
	} else {
		return 2;
	}
	return 0;
}
