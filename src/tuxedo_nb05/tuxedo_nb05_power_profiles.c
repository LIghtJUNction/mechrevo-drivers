// SPDX-License-Identifier: GPL-2.0+
/*!
 * Copyright (c) 2023-2024 TUXEDO Computers GmbH <tux@tuxedocomputers.com>
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
#include <linux/acpi.h>
#include <linux/module.h>
#include <linux/wmi.h>
#include <linux/version.h>
#include <linux/platform_device.h>
#include <linux/workqueue.h>
#include <linux/mutex.h>
#include <linux/spinlock.h>
#include "tuxedo_nb05_power_profiles.h"
#include "../tuxedo_compatibility_check/tuxedo_compatibility_check.h"

#define NB05_WMI_METHOD_BA_GUID	"99D89064-8D50-42BB-BEA9-155B2E5D0FCD"

/* Protects the WMI device, cached profile and firmware transactions. */
static DEFINE_MUTEX(nb05_wmi_aa_access_lock);
/* Notifications can queue work without re-entering the firmware mutex. */
static DEFINE_SPINLOCK(nb05_profile_work_lock);

struct driver_data_t {
	struct platform_device *pdev;
	u64 last_chosen_profile;
	bool stopping;
};

static struct wmi_device *__wmi_dev;
static bool profile_work_enabled;
static bool profile_changed_by_driver_flag;

static void nb05_rewrite_profile_work_handler(struct work_struct *work);
static DECLARE_DELAYED_WORK(nb05_rewrite_profile_work,
			    nb05_rewrite_profile_work_handler);

static void queue_profile_rewrite(unsigned long delay)
{
	unsigned long flags;

	spin_lock_irqsave(&nb05_profile_work_lock, flags);
	if (profile_work_enabled)
		mod_delayed_work(system_wq, &nb05_rewrite_profile_work, delay);
	spin_unlock_irqrestore(&nb05_profile_work_lock, flags);
}

/* Caller holds nb05_wmi_aa_access_lock. */
static int __nb05_wmi_aa_method(struct wmi_device *wdev, u32 wmi_method_id,
			       u64 *in, u64 *out)
{
	struct acpi_buffer acpi_buffer_in = { sizeof(*in), in };
	struct acpi_buffer return_buffer = { ACPI_ALLOCATE_BUFFER, NULL };
	union acpi_object *object;
	acpi_status status;
	int err = 0;

	status = wmidev_evaluate_method(wdev, 0, wmi_method_id,
				       &acpi_buffer_in, &return_buffer);
	if (ACPI_FAILURE(status)) {
		err = -EIO;
		goto out;
	}

	object = return_buffer.pointer;
	if (!object) {
		err = -ENODATA;
		goto out;
	}
	if (object->type != ACPI_TYPE_INTEGER) {
		err = -EIO;
		goto out;
	}
	*out = object->integer.value;
out:
	kfree(return_buffer.pointer);
	return err;
}

/* Caller holds nb05_wmi_aa_access_lock. */
static int nb05_wmi_aa_method(u32 wmi_method_id, u64 *in, u64 *out)
{
	struct driver_data_t *driver_data;

	if (!__wmi_dev)
		return -ENODEV;
	driver_data = dev_get_drvdata(&__wmi_dev->dev);
	if (driver_data->stopping)
		return -ENODEV;
	return __nb05_wmi_aa_method(__wmi_dev, wmi_method_id, in, out);
}

static int write_profile(u64 profile)
{
	u64 out;
	int err;

	WRITE_ONCE(profile_changed_by_driver_flag, true);
	err = nb05_wmi_aa_method(1, &profile, &out);
	if (!err && out)
		err = -EINVAL;
	if (err) {
		WRITE_ONCE(profile_changed_by_driver_flag, false);
		return err;
	}

	/* Firmware may emit another profile event shortly after a write. */
	queue_profile_rewrite(msecs_to_jiffies(300));
	return 0;
}

static int read_profile(u64 *profile)
{
	u64 in = 0;

	return nb05_wmi_aa_method(2, &in, profile);
}

struct char_to_value_t {
	const char *descriptor;
	u64 value;
};

static const struct char_to_value_t platform_profile_options[] = {
	{ .descriptor = "low-power",	.value = 2 },
	{ .descriptor = "balanced",	.value = 0 },
	{ .descriptor = "performance",	.value = 1 }
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
	u64 profile;
	int i, err;

	mutex_lock(&nb05_wmi_aa_access_lock);
	err = read_profile(&profile);
	mutex_unlock(&nb05_wmi_aa_access_lock);
	if (err)
		return err;

	for (i = 0; i < ARRAY_SIZE(platform_profile_options); ++i)
		if (platform_profile_options[i].value == profile)
			return sysfs_emit(buffer, "%s\n",
					  platform_profile_options[i].descriptor);

	return -EIO;
}

static void nb05_rewrite_profile_work_handler(struct work_struct *work)
{
	struct driver_data_t *driver_data;
	u64 current_profile;
	int err;

	mutex_lock(&nb05_wmi_aa_access_lock);
	if (!__wmi_dev)
		goto out;
	driver_data = dev_get_drvdata(&__wmi_dev->dev);
	if (driver_data->stopping)
		goto out;

	WRITE_ONCE(profile_changed_by_driver_flag, false);
	err = read_profile(&current_profile);
	if (!err && current_profile != driver_data->last_chosen_profile)
		err = write_profile(driver_data->last_chosen_profile);
	if (err)
		pr_err("failed to restore power profile: %d\n", err);
out:
	mutex_unlock(&nb05_wmi_aa_access_lock);
}

void rewrite_last_profile(void)
{
	queue_profile_rewrite(0);
}
EXPORT_SYMBOL(rewrite_last_profile);

bool profile_changed_by_driver(void)
{
	return READ_ONCE(profile_changed_by_driver_flag);
}
EXPORT_SYMBOL(profile_changed_by_driver);

static ssize_t platform_profile_store(struct device *dev,
				      struct device_attribute *attr,
				      const char *buffer, size_t size)
{
	struct driver_data_t *driver_data = dev_get_drvdata(dev);
	u64 profile;
	int i, err;

	for (i = 0; i < ARRAY_SIZE(platform_profile_options); ++i)
		if (sysfs_streq(buffer, platform_profile_options[i].descriptor))
			break;
	if (i == ARRAY_SIZE(platform_profile_options))
		return -EINVAL;

	profile = platform_profile_options[i].value;
	mutex_lock(&nb05_wmi_aa_access_lock);
	err = write_profile(profile);
	if (!err)
		driver_data->last_chosen_profile = profile;
	mutex_unlock(&nb05_wmi_aa_access_lock);

	if (err)
		return err;
	return size;
}

static DEVICE_ATTR_RO(platform_profile_choices);
static DEVICE_ATTR_RW(platform_profile);

static struct attribute *platform_profile_attrs[] = {
	&dev_attr_platform_profile_choices.attr,
	&dev_attr_platform_profile.attr,
	NULL
};

static const struct attribute_group platform_profile_attr_group = {
	.attrs = platform_profile_attrs
};

static void stop_profile_work(struct wmi_device *wdev)
{
	struct driver_data_t *driver_data = dev_get_drvdata(&wdev->dev);
	unsigned long flags;

	mutex_lock(&nb05_wmi_aa_access_lock);
	driver_data->stopping = true;
	spin_lock_irqsave(&nb05_profile_work_lock, flags);
	profile_work_enabled = false;
	spin_unlock_irqrestore(&nb05_profile_work_lock, flags);
	WRITE_ONCE(profile_changed_by_driver_flag, false);
	mutex_unlock(&nb05_wmi_aa_access_lock);

	/* No producer can re-arm work after profile_work_enabled was cleared. */
	cancel_delayed_work_sync(&nb05_rewrite_profile_work);

	mutex_lock(&nb05_wmi_aa_access_lock);
	if (__wmi_dev == wdev)
		__wmi_dev = NULL;
	mutex_unlock(&nb05_wmi_aa_access_lock);
}

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 3, 0)
static int tuxedo_nb05_power_profiles_probe(struct wmi_device *wdev)
#else
static int tuxedo_nb05_power_profiles_probe(struct wmi_device *wdev, const void *dummy_context)
#endif
{
	const struct platform_device_info pinfo = {
		.name = "tuxedo_platform_profile",
		.id = PLATFORM_DEVID_NONE,
		.parent = &wdev->dev
	};
	struct driver_data_t *driver_data;
	unsigned long flags;
	u64 in = 0;
	int err;

	if (!tuxedo_is_compatible() || !wmi_has_guid(NB05_WMI_METHOD_BA_GUID))
		return -ENODEV;

	driver_data = devm_kzalloc(&wdev->dev, sizeof(*driver_data), GFP_KERNEL);
	if (!driver_data)
		return -ENOMEM;

	dev_set_drvdata(&wdev->dev, driver_data);
	driver_data->pdev = platform_device_register_full(&pinfo);
	if (IS_ERR(driver_data->pdev))
		return PTR_ERR(driver_data->pdev);
	platform_set_drvdata(driver_data->pdev, driver_data);

	mutex_lock(&nb05_wmi_aa_access_lock);
	if (__wmi_dev) {
		err = -EBUSY;
		goto out_unlock;
	}

	err = __nb05_wmi_aa_method(wdev, 2, &in, &driver_data->last_chosen_profile);
	if (err)
		goto out_unlock;

	__wmi_dev = wdev;
	spin_lock_irqsave(&nb05_profile_work_lock, flags);
	profile_work_enabled = true;
	spin_unlock_irqrestore(&nb05_profile_work_lock, flags);
out_unlock:
	mutex_unlock(&nb05_wmi_aa_access_lock);
	if (err)
		goto unregister_device;

	err = sysfs_create_group(&driver_data->pdev->dev.kobj,
				 &platform_profile_attr_group);
	if (!err)
		return 0;

	stop_profile_work(wdev);
unregister_device:
	platform_device_unregister(driver_data->pdev);
	return err;
}

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
static int tuxedo_nb05_power_profiles_remove(struct wmi_device *wdev)
#else
static void tuxedo_nb05_power_profiles_remove(struct wmi_device *wdev)
#endif
{
	struct driver_data_t *driver_data = dev_get_drvdata(&wdev->dev);

	sysfs_remove_group(&driver_data->pdev->dev.kobj, &platform_profile_attr_group);
	stop_profile_work(wdev);
	platform_device_unregister(driver_data->pdev);

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
	return 0;
#endif
}

static const struct wmi_device_id tuxedo_nb05_power_profiles_device_ids[] = {
	{ .guid_string = NB05_WMI_METHOD_BA_GUID },
	{ }
};

static struct wmi_driver tuxedo_nb05_power_profiles_driver = {
	.driver = {
		.name = "tuxedo_nb05_power_profiles",
		.owner = THIS_MODULE
	},
	.id_table = tuxedo_nb05_power_profiles_device_ids,
	.probe = tuxedo_nb05_power_profiles_probe,
	.remove = tuxedo_nb05_power_profiles_remove,
};

module_wmi_driver(tuxedo_nb05_power_profiles_driver);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("Driver for NB05 power profiles");
MODULE_LICENSE("GPL");

MODULE_DEVICE_TABLE(wmi, tuxedo_nb05_power_profiles_device_ids);
MODULE_ALIAS("wmi:" NB05_WMI_METHOD_BA_GUID);
