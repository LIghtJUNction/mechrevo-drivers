/* The injected driver functions use no hardware or kernel services here. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/types.h>

typedef uint8_t u8;
typedef uint16_t u16;
#define __init
#define __unused __attribute__((unused))
enum { DMI_SYS_VENDOR, DMI_BOARD_NAME };
struct device { int unused; };
struct device_attribute { int unused; };

/* @DRIVER_CONSTANTS@ */

static const char *system_vendor = "OTHER";
static const char *board_name = "OTHER";
static int dmi_match(int field, const char *expected)
{
	const char *actual = field == DMI_SYS_VENDOR ? system_vendor : board_name;
	return actual && strcmp(actual, expected) == 0;
}

static int parse_status, parse_calls;
static u8 parse_value;
static int kstrtou8(const char *buffer, unsigned int base, u8 *value)
{
	assert(buffer && base == 0);
	++parse_calls;
	if (parse_status)
		return parse_status;
	*value = parse_value;
	return 0;
}

static int read_status, read_calls, write_status, write_calls, fail_write_number;
static u16 read_address, write_addresses[4];
static u8 read_value, write_values[4];
static int uniwill_read_ec_ram(u16 address, u8 *value)
{
	++read_calls;
	read_address = address;
	if (read_status)
		return read_status;
	*value = read_value;
	return 0;
}
static int uniwill_write_ec_ram(u16 address, u8 value)
{
	assert(write_calls < 4);
	write_addresses[write_calls] = address;
	write_values[write_calls++] = value;
	if (write_status && (!fail_write_number || write_calls == fail_write_number))
		return write_status;
	return 0;
}

/* Kernel callbacks retain unused device/attribute parameters in their ABI. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
/* @DRIVER_FUNCTIONS@ */
#pragma GCC diagnostic pop

static void reset_mock(void)
{
	parse_status = parse_calls = read_status = read_calls = 0;
	write_status = write_calls = fail_write_number = 0;
	parse_value = read_value = 0;
	read_address = 0;
	memset(write_addresses, 0, sizeof(write_addresses));
	memset(write_values, 0, sizeof(write_values));
}

static void model(bool verified)
{
	system_vendor = verified ? "MECHREVO" : "OTHER";
	board_name = "GM6PX0X";
}

static void check_legacy_writes(int count)
{
	const u16 addresses[] = {0x0743, 0x0744, 0x0745, 0x0746};
	const u8 values[] = {7, 0, 255, 25};
	assert(write_calls == count && read_calls == 0);
	for (int i = 0; i < count; ++i) {
		assert(write_addresses[i] == addresses[i]);
		assert(write_values[i] == values[i]);
	}
}

int main(int argc, char **argv)
{
	assert(argc == 2);
	const char *test = argv[1];
	const size_t count = 3;
	const u8 values[] = {0, 25, 255};

	if (strcmp(test, "dmi_boundaries") == 0) {
		const struct { const char *vendor, *board; bool preserve; } cases[] = {
			{"MECHREVO", "GM6PX0X", true},
			{"OTHER", "GM6PX0X", false},
			{"MECHREVO", "GM6PX0", false},
			{"MECHREVO", "GM6PX0X-extra", false},
			{"MECHREVO-extra", "GM6PX0X", false},
			{"mechrevo", "GM6PX0X", false},
			{"MECHREVO", "gm6px0x", false},
			{NULL, "GM6PX0X", false},
			{"MECHREVO", NULL, false},
		};
		for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
			reset_mock();
			system_vendor = cases[i].vendor;
			board_name = cases[i].board;
			assert(uw_preserve_firmware_power_config() == cases[i].preserve);
			assert(init_db_and_ctgp() == 0);
			check_legacy_writes(cases[i].preserve ? 0 : 4);
		}
	}
	else if (strcmp(test, "preserve_config") == 0) {
		model(true);
		read_status = -EIO;
		write_status = -EIO;
		assert(init_db_and_ctgp() == 0);
		assert(read_calls == 0 && write_calls == 0);
	}
	else if (strcmp(test, "legacy_config") == 0) {
		assert(init_db_and_ctgp() == 0);
		check_legacy_writes(4);
	}
	else if (strcmp(test, "legacy_write_errors") == 0) {
		for (int i = 1; i <= 4; ++i) {
			reset_mock();
			write_status = -ETIMEDOUT;
			fail_write_number = i;
			assert(init_db_and_ctgp() == -ETIMEDOUT);
			check_legacy_writes(i);
		}
	}
	else if (strcmp(test, "ctgp_unchanged") == 0 || strcmp(test, "ctgp_changed") == 0) {
		bool unchanged = strcmp(test, "ctgp_unchanged") == 0;
		for (int verified = 0; verified <= 1; ++verified) {
			model(verified);
			for (size_t i = 0; i < sizeof(values) / sizeof(values[0]); ++i) {
				reset_mock();
				parse_value = values[i];
				read_value = unchanged ? parse_value : (u8)(parse_value + 1);
				assert(ctgp_offset_store(NULL, NULL, "25\n", count) == (ssize_t)count);
				assert(parse_calls == 1 && read_calls == 1);
				assert(read_address == 0x0744);
				assert(write_calls == (unchanged ? 0 : 1));
				if (!unchanged) {
					assert(write_addresses[0] == 0x0744);
					assert(write_values[0] == parse_value);
				}
			}
		}
	}
	else if (strcmp(test, "ctgp_read_error") == 0) {
		parse_value = 25;
		read_status = -ETIMEDOUT;
		assert(ctgp_offset_store(NULL, NULL, "25\n", count) == -ETIMEDOUT);
		assert(parse_calls == 1 && read_calls == 1 && write_calls == 0);
		assert(read_address == 0x0744);
	}
	else if (strcmp(test, "ctgp_write_error") == 0) {
		parse_value = 25;
		write_status = -EIO;
		assert(ctgp_offset_store(NULL, NULL, "25\n", count) == -EIO);
		assert(parse_calls == 1 && read_calls == 1 && write_calls == 1);
		assert(read_address == 0x0744 && write_addresses[0] == 0x0744);
		assert(write_values[0] == 25);
	}
	else if (strcmp(test, "ctgp_parse_error") == 0) {
		const int errors[] = {-EINVAL, -ERANGE};
		for (size_t i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
			reset_mock();
			parse_status = errors[i];
			assert(ctgp_offset_store(NULL, NULL, "invalid", count) == errors[i]);
			assert(parse_calls == 1 && read_calls == 0 && write_calls == 0);
		}
	}
	else
		return 2;
	return 0;
}
