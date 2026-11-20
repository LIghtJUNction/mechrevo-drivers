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
#include <linux/acpi.h>
#include <linux/module.h>
#include <linux/wmi.h>
#include <linux/version.h>
#include "../tuxedo_compatibility_check/tuxedo_compatibility_check.h"
#include "tuxedo_nb04_wmi_bs.h"

#define BS_INPUT_BUFFER_LENGTH	8
#define BS_OUTPUT_BUFFER_LENGTH	80

static DEFINE_MUTEX(nb04_wmi_bs_access_lock);

static struct wmi_device *__wmi_dev;

bool nb04_wmi_bs_available(void)
{
	bool available;

	mutex_lock(&nb04_wmi_bs_access_lock);
	available = __wmi_dev != NULL;
	mutex_unlock(&nb04_wmi_bs_access_lock);
	return available;
}
EXPORT_SYMBOL(nb04_wmi_bs_available);

/* Method interface: 8 bytes in, 80 bytes out. */
int nb04_wmi_bs_method(u32 wmi_method_id, u8 *in, u8 *out)
{
	struct acpi_buffer acpi_buffer_in = { BS_INPUT_BUFFER_LENGTH, in };
	struct acpi_buffer return_buffer = { ACPI_ALLOCATE_BUFFER, NULL };
	union acpi_object *object;
	acpi_status status;
	int err = 0;

	if (!in || !out)
		return -EINVAL;

	mutex_lock(&nb04_wmi_bs_access_lock);
	if (!__wmi_dev) {
		err = -ENODEV;
		goto out;
	}

	pr_debug("evaluate: %u\n", wmi_method_id);
	status = wmidev_evaluate_method(__wmi_dev, 0, wmi_method_id,
				       &acpi_buffer_in, &return_buffer);
	if (ACPI_FAILURE(status)) {
		pr_err("failed to evaluate wmi method %u\n", wmi_method_id);
		err = -EIO;
		goto out;
	}

	object = return_buffer.pointer;
	if (!object) {
		err = -ENODATA;
		goto out;
	}
	if (object->type != ACPI_TYPE_BUFFER) {
		/* Firmware returns integer zero for an unknown method. */
		err = -EINVAL;
		goto out;
	}
	if (object->buffer.length != BS_OUTPUT_BUFFER_LENGTH ||
	    !object->buffer.pointer) {
		err = -EIO;
		goto out;
	}

	memcpy(out, object->buffer.pointer, BS_OUTPUT_BUFFER_LENGTH);
out:
	kfree(return_buffer.pointer);
	mutex_unlock(&nb04_wmi_bs_access_lock);
	return err;
}
EXPORT_SYMBOL(nb04_wmi_bs_method);

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 3, 0)
static int tuxedo_nb04_wmi_probe(struct wmi_device *wdev)
#else
static int tuxedo_nb04_wmi_probe(struct wmi_device *wdev, const void *dummy_context)
#endif
{
	int err = 0;

	pr_debug("driver probe\n");

	if (!tuxedo_is_compatible())
		return -ENODEV;

	if (!wmi_has_guid(NB04_WMI_BS_GUID))
		return -ENODEV;

	mutex_lock(&nb04_wmi_bs_access_lock);
	if (__wmi_dev)
		err = -EBUSY;
	else
		__wmi_dev = wdev;
	mutex_unlock(&nb04_wmi_bs_access_lock);

	return err;
}

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
static int tuxedo_nb04_wmi_remove(struct wmi_device *wdev)
#else
static void tuxedo_nb04_wmi_remove(struct wmi_device *wdev)
#endif
{
	mutex_lock(&nb04_wmi_bs_access_lock);
	if (__wmi_dev == wdev)
		__wmi_dev = NULL;
	mutex_unlock(&nb04_wmi_bs_access_lock);
	pr_debug("driver remove\n");

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
	return 0;
#endif
}

static const struct wmi_device_id tuxedo_nb04_wmi_device_ids[] = {
	{ .guid_string = NB04_WMI_BS_GUID },
	{ }
};

static struct wmi_driver tuxedo_nb04_wmi_driver = {
	.driver = {
		.name = "tuxedo_nb04_wmi",
		.owner = THIS_MODULE
	},
	.id_table = tuxedo_nb04_wmi_device_ids,
	.probe = tuxedo_nb04_wmi_probe,
	.remove = tuxedo_nb04_wmi_remove,
};

module_wmi_driver(tuxedo_nb04_wmi_driver);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("Driver for NB04 WMI BS methods");
MODULE_LICENSE("GPL");

MODULE_DEVICE_TABLE(wmi, tuxedo_nb04_wmi_device_ids);
