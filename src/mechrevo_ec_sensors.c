// SPDX-License-Identifier: GPL-2.0+
/* Read-only EC temperatures verified on this exact machine/firmware only. */
#include <linux/dmi.h>
#include <linux/err.h>
#include <linux/hwmon.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/string.h>

#include "uniwill_interfaces.h"

#define MECHREVO_CPU_EC_TEMP 0x043e
#define MECHREVO_GPU_EC_TEMP 0x044f

static struct platform_device *mechrevo_sensors_device;
static struct device *mechrevo_sensors_hwmon;

static const struct dmi_system_id mechrevo_sensors_dmi[] = {
	{
		.matches = {
			DMI_EXACT_MATCH(DMI_SYS_VENDOR, "MECHREVO"),
			DMI_EXACT_MATCH(DMI_PRODUCT_NAME, "Kuangshi16Pro Series GM6PX0X"),
			DMI_EXACT_MATCH(DMI_BIOS_VERSION, "N.1.09MRO07"),
		},
	},
	{ }
};
MODULE_DEVICE_TABLE(dmi, mechrevo_sensors_dmi);

static bool mechrevo_sensors_supported(void)
{
	/* dmi_match compares the whole field, unlike substring DMI table matches. */
	return dmi_match(DMI_SYS_VENDOR, "MECHREVO") &&
		dmi_match(DMI_PRODUCT_NAME, "Kuangshi16Pro Series GM6PX0X") &&
		dmi_match(DMI_BIOS_VERSION, "N.1.09MRO07");
}

static int mechrevo_sensors_temperature(int channel, long *value)
{
	u8 raw;
	int ret;
	u16 address;

	if (channel == 0)
		address = MECHREVO_CPU_EC_TEMP;
	else if (channel == 1)
		address = MECHREVO_GPU_EC_TEMP;
	else
		return -EOPNOTSUPP;

	ret = uniwill_read_ec_ram(address, &raw);
	if (ret)
		return ret < 0 ? ret : -EIO;
	if (raw > 125)
		return -ENODATA;

	*value = (long)raw * 1000;
	return 0;
}

static umode_t mechrevo_sensors_is_visible(const void *data,
					 enum hwmon_sensor_types type,
					 u32 attr, int channel)
{
	if (type != hwmon_temp || channel < 0 || channel > 1)
		return 0;
	return attr == hwmon_temp_input || attr == hwmon_temp_label ? 0444 : 0;
}

static int mechrevo_sensors_read(struct device *dev,
				enum hwmon_sensor_types type,
				u32 attr, int channel, long *value)
{
	if (type != hwmon_temp || attr != hwmon_temp_input)
		return -EOPNOTSUPP;
	return mechrevo_sensors_temperature(channel, value);
}

static int mechrevo_sensors_read_string(struct device *dev,
				       enum hwmon_sensor_types type,
				       u32 attr, int channel,
				       const char **value)
{
	if (type != hwmon_temp || attr != hwmon_temp_label ||
	    channel < 0 || channel > 1)
		return -EOPNOTSUPP;
	*value = channel == 0 ? "CPU EC" : "GPU EC";
	return 0;
}

static const struct hwmon_ops mechrevo_sensors_ops = {
	.is_visible = mechrevo_sensors_is_visible,
	.read = mechrevo_sensors_read,
	.read_string = mechrevo_sensors_read_string,
};

static const struct hwmon_channel_info * const mechrevo_sensors_info[] = {
	HWMON_CHANNEL_INFO(temp,
		HWMON_T_INPUT | HWMON_T_LABEL,
		HWMON_T_INPUT | HWMON_T_LABEL),
	NULL,
};

static const struct hwmon_chip_info mechrevo_sensors_chip = {
	.ops = &mechrevo_sensors_ops,
	.info = mechrevo_sensors_info,
};

static int __init mechrevo_sensors_init(void)
{
	char *interface_id = NULL;
	long value;
	int ret;

	if (!mechrevo_sensors_supported())
		return -ENODEV;
	ret = uniwill_get_active_interface_id(&interface_id);
	if (ret)
		return ret < 0 ? ret : -EIO;
	if (!interface_id || strcmp(interface_id, UNIWILL_INTERFACE_WMI_STRID))
		return -ENODEV;
	ret = mechrevo_sensors_temperature(0, &value);
	if (ret)
		return ret;
	ret = mechrevo_sensors_temperature(1, &value);
	if (ret)
		return ret;

	mechrevo_sensors_device = platform_device_register_simple(
		"mechrevo_ec_sensors", PLATFORM_DEVID_NONE, NULL, 0);
	if (IS_ERR(mechrevo_sensors_device)) {
		ret = PTR_ERR(mechrevo_sensors_device);
		mechrevo_sensors_device = NULL;
		return ret;
	}
	mechrevo_sensors_hwmon = hwmon_device_register_with_info(
		&mechrevo_sensors_device->dev, "mechrevo_ec", NULL,
		&mechrevo_sensors_chip, NULL);
	if (IS_ERR(mechrevo_sensors_hwmon)) {
		ret = PTR_ERR(mechrevo_sensors_hwmon);
		mechrevo_sensors_hwmon = NULL;
		platform_device_unregister(mechrevo_sensors_device);
		mechrevo_sensors_device = NULL;
		return ret;
	}
	return 0;
}

static void __exit mechrevo_sensors_exit(void)
{
	hwmon_device_unregister(mechrevo_sensors_hwmon);
	platform_device_unregister(mechrevo_sensors_device);
}

module_init(mechrevo_sensors_init);
module_exit(mechrevo_sensors_exit);
MODULE_SOFTDEP("pre: uniwill_wmi");
MODULE_DESCRIPTION("Read-only MECHREVO GM6PX0X EC temperatures (N.1.09MRO07)");
MODULE_LICENSE("GPL");
