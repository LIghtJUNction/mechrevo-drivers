// SPDX-License-Identifier: GPL-2.0+
/*!
 * Copyright (c) 2024-2026 TUXEDO Computers GmbH <tux@tuxedocomputers.com>
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
#include <linux/acpi.h>
#include <linux/version.h>
#include <linux/limits.h>
#include <linux/mutex.h>
#include "tuxi_acpi.h"

#define DRIVER_NAME "tuxi_acpi"

struct tuxi_platform_driver_data_t {
	acpi_handle tfan_handle;
};

static DEFINE_MUTEX(tuxi_lock);
static struct tuxi_platform_driver_data_t *tuxi_driver_data;

static int evaluate_intparams(acpi_handle handle, acpi_string pathname,
			      const unsigned long long *int_params, u32 param_count,
			      unsigned long long *retval)
{
	/* TFAN methods take at most a fan index and a value. */
	union acpi_object params[2] = { };
	struct acpi_object_list input = { param_count, params };
	unsigned long long result;
	acpi_status status;
	int i;

	if (!handle)
		return -ENODEV;
	if (param_count > ARRAY_SIZE(params) || (param_count && !int_params))
		return -EINVAL;

	for (i = 0; i < param_count; ++i) {
		params[i].type = ACPI_TYPE_INTEGER;
		params[i].integer.value = int_params[i];
	}

	status = acpi_evaluate_integer(handle, pathname,
				      param_count ? &input : NULL, &result);
	if (ACPI_FAILURE(status))
		return -EIO;

	if (retval)
		*retval = result;
	return 0;
}

static int tuxi_evaluate(acpi_string pathname,
			 const unsigned long long *args, u32 count,
			 unsigned long long *retval)
{
	int err;

	mutex_lock(&tuxi_lock);
	if (!tuxi_driver_data)
		err = -ENODEV;
	else
		err = evaluate_intparams(tuxi_driver_data->tfan_handle, pathname,
					args, count, retval);
	mutex_unlock(&tuxi_lock);
	return err;
}

int tuxi_set_fan_speed(u8 fan_index, u8 fan_speed)
{
	unsigned long long retval;
	unsigned long long args[] = { fan_index, fan_speed };
	int err;

	err = tuxi_evaluate("SSPD",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if (retval)
		return -EINVAL;

	return 0;
}
EXPORT_SYMBOL(tuxi_set_fan_speed);

int tuxi_get_fan_speed(u8 fan_index, u8 *fan_speed)
{
	unsigned long long retval;
	unsigned long long args[] = { fan_index };
	int err;

	if (!fan_speed)
		return -EINVAL;

	err = tuxi_evaluate("GSPD",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if (retval > U8_MAX)
		return -ERANGE;

	*fan_speed = (u8) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_fan_speed);

int tuxi_get_nr_fans(u8 *nr_fans)
{
	unsigned long long retval;
	int err;

	if (!nr_fans)
		return -EINVAL;

	err = tuxi_evaluate("GCNT",
				 NULL, 0,
				 &retval);
	if (err)
		return err;

	if (retval > U8_MAX)
		return -ERANGE;

	*nr_fans = (u8) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_nr_fans);

int tuxi_set_fan_mode(enum tuxi_fan_mode mode)
{
	unsigned long long retval;
	unsigned long long args[] = { mode };
	int err;

	if (mode != AUTO && mode != MANUAL)
		return -EINVAL;

	err = tuxi_evaluate("SMOD",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if ((s64)retval < 0)
		return -EINVAL;

	return 0;
}
EXPORT_SYMBOL(tuxi_set_fan_mode);

int tuxi_get_fan_mode(enum tuxi_fan_mode *mode)
{
	unsigned long long retval;
	int err;

	if (!mode)
		return -EINVAL;

	err = tuxi_evaluate("GMOD",
				 NULL, 0,
				 &retval);
	if (err)
		return err;

	if (retval > MANUAL)
		return -EINVAL;

	*mode = (u8) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_fan_mode);

int tuxi_get_fan_type(u8 fan_index, enum tuxi_fan_type *type)
{
	unsigned long long retval;
	unsigned long long args[] = { fan_index };
	int err;

	if (!type)
		return -EINVAL;

	err = tuxi_evaluate("GTYP",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if (retval > GPU)
		return -EINVAL;

	*type = (u8) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_fan_type);

int tuxi_get_fan_temp(u8 index, u16 *temp)
{
	unsigned long long retval;
	unsigned long long args[] = { index };
	int err;

	if (!temp)
		return -EINVAL;

	err = tuxi_evaluate("GTMP",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if (retval > U16_MAX)
		return -ERANGE;

	*temp = (u16) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_fan_temp);

int tuxi_get_fan_rpm(u8 index, u16 *rpm)
{
	unsigned long long retval;
	unsigned long long args[] = { index };
	int err;

	if (!rpm)
		return -EINVAL;

	err = tuxi_evaluate("GRPM",
				 args, ARRAY_SIZE(args),
				 &retval);
	if (err)
		return err;

	if (retval > U16_MAX)
		return -ERANGE;

	*rpm = (u16) retval;

	return 0;
}
EXPORT_SYMBOL(tuxi_get_fan_rpm);

static int get_tfan(struct acpi_device *tuxi_dev, acpi_handle *tfan_handle)
{
	acpi_status status;
	status = acpi_get_handle(tuxi_dev->handle, "TFAN", tfan_handle);
	if (ACPI_FAILURE(status))
		return -ENODEV;
	return 0;
}

static int tuxi_platform_probe(struct platform_device *pdev)
{
	struct tuxi_platform_driver_data_t *driver_data;
	struct acpi_device *acpi;
	int err;

	acpi = ACPI_COMPANION(&pdev->dev);
	if (!acpi)
		return -ENODEV;

	driver_data = devm_kzalloc(&pdev->dev, sizeof(*driver_data), GFP_KERNEL);
	if (!driver_data)
		return -ENOMEM;

	err = get_tfan(acpi, &driver_data->tfan_handle);
	if (err)
		return err;

	platform_set_drvdata(pdev, driver_data);
	mutex_lock(&tuxi_lock);
	if (tuxi_driver_data)
		err = -EBUSY;
	else
		tuxi_driver_data = driver_data;
	mutex_unlock(&tuxi_lock);
	if (err)
		return err;

	pr_info("interface initialized\n");

	return 0;
}

#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 11, 0)
static int tuxi_platform_remove(struct platform_device *pdev)
#else
static void tuxi_platform_remove(struct platform_device *pdev)
#endif
{
	mutex_lock(&tuxi_lock);
	if (tuxi_driver_data == platform_get_drvdata(pdev))
		tuxi_driver_data = NULL;
	mutex_unlock(&tuxi_lock);
	pr_debug("driver remove\n");

#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 11, 0)
	return 0;
#endif
}

static int driver_suspend_callb(struct device *dev)
{
	pr_debug("driver suspend\n");
	return 0;
}

static int driver_resume_callb(struct device *dev)
{
	pr_debug("driver resume\n");
	return 0;
}

static DEFINE_SIMPLE_DEV_PM_OPS(tuxi_driver_pm_ops, driver_suspend_callb, driver_resume_callb);

static const struct acpi_device_id tuxi_acpi_device_ids[] = {
	{ TUXI_ACPI_RESOURCE_HID, 0 },
	{ "", 0 }
};

static struct platform_driver tuxi_platform_driver = {
	.driver = {
		.name = DRIVER_NAME,
		.acpi_match_table = tuxi_acpi_device_ids,
		.pm = pm_ptr(&tuxi_driver_pm_ops),
	},
	.probe = tuxi_platform_probe,
	.remove = tuxi_platform_remove,
};

module_platform_driver(tuxi_platform_driver);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("Driver for TUXEDO ACPI interface");
MODULE_LICENSE("GPL");

MODULE_DEVICE_TABLE(acpi, tuxi_acpi_device_ids);
