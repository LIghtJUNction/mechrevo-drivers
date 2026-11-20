// SPDX-License-Identifier: GPL-2.0+
/*!
 * Copyright (c) 2023 TUXEDO Computers GmbH <tux@tuxedocomputers.com>
 *
 * This file is part of tuxedo-drivers.
 *
 * This program is free software; you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation; either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License along
 * with this program; if not, see <https://www.gnu.org/licenses/>.
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/mutex.h>
#include <linux/dmi.h>
#include <linux/version.h>
#include "tuxedo_nb04_wmi_bs.h"

#define DEFAULT_PROFILE		WMI_SYSTEM_MODE_BEAST

struct driver_data_t {
	struct mutex lock;
	u8 current_profile_value;
};

static int set_system_mode(u8 mode_input)
{
	int err, wmi_return;
	u8 in[BS_INPUT_BUFFER_LENGTH] = { 0 };
	u8 out[BS_OUTPUT_BUFFER_LENGTH] = { 0 };

	if (mode_input >= WMI_SYSTEM_MODE_END)
		return -EINVAL;

	in[0] = mode_input;

	err = nb04_wmi_bs_method(0x07, in, out);
	if (err)
		return err;

	wmi_return = (out[1] << 8) | out[0];
	if (wmi_return != WMI_RETURN_STATUS_SUCCESS)
		return -EIO;

	return 0;
}

static ssize_t platform_profile_choices_show(struct device *dev,
					     struct device_attribute *attr,
					     char *buffer);

static ssize_t platform_profile_show(struct device *dev,
				     struct device_attribute *attr, char *buffer);

static ssize_t platform_profile_store(struct device *dev,
				      struct device_attribute *attr,
				      const char *buffer, size_t size);

struct platform_profile_attrs_t {
	struct device_attribute platform_profile_choices;
	struct device_attribute platform_profile;
};

struct platform_profile_attrs_t platform_profile_attrs = {
	.platform_profile_choices = __ATTR(platform_profile_choices, 0444,
					   platform_profile_choices_show, NULL),
	.platform_profile = __ATTR(platform_profile, 0644,
				   platform_profile_show, platform_profile_store)
};

static struct attribute *platform_profile_attrs_list[] = {
	&platform_profile_attrs.platform_profile_choices.attr,
	&platform_profile_attrs.platform_profile.attr,
	NULL
};

static struct attribute_group platform_profile_attr_group = {
	.attrs = platform_profile_attrs_list
};

struct char_to_value_t {
	const char *descriptor;
	u64 value;
};

static const struct char_to_value_t platform_profile_options[] = {
	{ .descriptor = "low-power",	.value = WMI_SYSTEM_MODE_BATTERY },
	{ .descriptor = "balanced",	.value = WMI_SYSTEM_MODE_HUMAN },
	{ .descriptor = "performance",	.value = WMI_SYSTEM_MODE_BEAST },
};

static ssize_t platform_profile_choices_show(struct device *dev,
					     struct device_attribute *attr,
					     char *buffer)
{
	return sysfs_emit(buffer, "%s %s %s\n",
			  platform_profile_options[0].descriptor,
			  platform_profile_options[1].descriptor,
			  platform_profile_options[2].descriptor);
}

static ssize_t platform_profile_show(struct device *dev,
				     struct device_attribute *attr, char *buffer)
{
	u8 platform_profile_value;
	int i;
	struct driver_data_t *driver_data = dev_get_drvdata(dev);

	mutex_lock(&driver_data->lock);
	platform_profile_value = driver_data->current_profile_value;
	mutex_unlock(&driver_data->lock);

	for (i = 0; i < ARRAY_SIZE(platform_profile_options); ++i)
		if (platform_profile_options[i].value == platform_profile_value)
			return sysfs_emit(buffer, "%s\n",
					  platform_profile_options[i].descriptor);

	return -EIO;
}

static ssize_t platform_profile_store(struct device *dev,
				      struct device_attribute *attr,
				      const char *buffer, size_t size)
{
	struct driver_data_t *driver_data = dev_get_drvdata(dev);
	u8 profile;
	int i, err;

	for (i = 0; i < ARRAY_SIZE(platform_profile_options); ++i)
		if (sysfs_streq(buffer, platform_profile_options[i].descriptor))
			break;
	if (i == ARRAY_SIZE(platform_profile_options))
		return -EINVAL;

	profile = platform_profile_options[i].value;
	mutex_lock(&driver_data->lock);
	err = set_system_mode(profile);
	if (!err)
		driver_data->current_profile_value = profile;
	mutex_unlock(&driver_data->lock);

	if (err)
		return err;
	return size;
}

static int __init tuxedo_nb04_power_profiles_probe(struct platform_device *pdev)
{
	int err;
	struct driver_data_t *driver_data = devm_kzalloc(&pdev->dev, sizeof(*driver_data), GFP_KERNEL);
	if (!driver_data)
		return -ENOMEM;

	pr_debug("driver probe\n");

	// Sirius uses other platform control interface
	if (dmi_match(DMI_SYS_VENDOR, "TUXEDO") &&
	    dmi_match(DMI_PRODUCT_SKU, "SIRIUS1601"))
		return -ENODEV;

	dev_set_drvdata(&pdev->dev, driver_data);

	mutex_init(&driver_data->lock);
	err = set_system_mode(DEFAULT_PROFILE);
	if (err)
		return err;
	driver_data->current_profile_value = DEFAULT_PROFILE;

	return devm_device_add_group(&pdev->dev, &platform_profile_attr_group);
}

static struct platform_device *tuxedo_nb04_power_profiles_device;
static struct platform_driver tuxedo_nb04_power_profiles_driver = {
	.driver.name = "tuxedo_platform_profile",
};

static int __init tuxedo_nb04_power_profiles_init(void)
{
	if (!nb04_wmi_bs_available())
		return -ENODEV;

	tuxedo_nb04_power_profiles_device =
		platform_create_bundle(&tuxedo_nb04_power_profiles_driver,
				       tuxedo_nb04_power_profiles_probe, NULL, 0, NULL, 0);

	if (IS_ERR(tuxedo_nb04_power_profiles_device))
		return PTR_ERR(tuxedo_nb04_power_profiles_device);

	return 0;
}

static void __exit tuxedo_nb04_power_profiles_exit(void)
{
	platform_device_unregister(tuxedo_nb04_power_profiles_device);
	platform_driver_unregister(&tuxedo_nb04_power_profiles_driver);
}

module_init(tuxedo_nb04_power_profiles_init);
module_exit(tuxedo_nb04_power_profiles_exit);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("TUXEDO Computers NB04 platform profile driver");
MODULE_LICENSE("GPL");
