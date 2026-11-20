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
#include <linux/delay.h>
#include "tuxedo_nb04_wmi_ab.h"
#include "../tuxedo_compatibility_check/tuxedo_compatibility_check.h"

/* Serializes firmware calls with publication/removal of the WMI device. */
static DEFINE_MUTEX(nb04_wmi_ab_lock);
static struct wmi_device *__wmi_dev;

static int nb04_wmi_ab_evaluate(u32 wmi_method_id, u8 *in,
			       acpi_size input_length, u8 *out,
			       size_t output_length, u64 *integer_out)
{
	struct acpi_buffer acpi_buffer_in = { input_length, in };
	struct acpi_buffer return_buffer = { ACPI_ALLOCATE_BUFFER, NULL };
	union acpi_object *object;
	acpi_status status;
	int err = 0;

	if (!in || (!out && !integer_out))
		return -EINVAL;

	mutex_lock(&nb04_wmi_ab_lock);
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

	if (integer_out) {
		if (object->type != ACPI_TYPE_INTEGER) {
			err = -EIO;
			goto out;
		}
		*integer_out = object->integer.value;
	} else {
		if (object->type != ACPI_TYPE_BUFFER ||
		    object->buffer.length != output_length ||
		    !object->buffer.pointer) {
			err = -EIO;
			goto out;
		}
		memcpy(out, object->buffer.pointer, output_length);
	}

out:
	kfree(return_buffer.pointer);
	mutex_unlock(&nb04_wmi_ab_lock);
	return err;
}

/* Method interface: 8 bytes in, 80 bytes out. */
int nb04_wmi_ab_method_buffer(u32 wmi_method_id, u8 *in, u8 *out)
{
	return nb04_wmi_ab_evaluate(wmi_method_id, in,
				   AB_INPUT_BUFFER_LENGTH_NORMAL, out,
				   AB_OUTPUT_BUFFER_LENGTH, NULL);
}
EXPORT_SYMBOL(nb04_wmi_ab_method_buffer);

/* Method interface: 8 bytes in, 10 bytes out. */
int nb04_wmi_ab_method_buffer_reduced_output(u32 wmi_method_id, u8 *in, u8 *out)
{
	return nb04_wmi_ab_evaluate(wmi_method_id, in,
				   AB_INPUT_BUFFER_LENGTH_NORMAL, out,
				   AB_OUTPUT_BUFFER_LENGTH_REDUCED, NULL);
}
EXPORT_SYMBOL(nb04_wmi_ab_method_buffer_reduced_output);

/* Method interface: 496 bytes in, 80 bytes out. */
int nb04_wmi_ab_method_extended_input(u32 wmi_method_id, u8 *in, u8 *out)
{
	return nb04_wmi_ab_evaluate(wmi_method_id, in,
				   AB_INPUT_BUFFER_LENGTH_EXTENDED, out,
				   AB_OUTPUT_BUFFER_LENGTH, NULL);
}
EXPORT_SYMBOL(nb04_wmi_ab_method_extended_input);

/* Method interface: 8 bytes in, integer out. */
int nb04_wmi_ab_method_int_out(u32 wmi_method_id, u8 *in, u64 *out)
{
	return nb04_wmi_ab_evaluate(wmi_method_id, in,
				   AB_INPUT_BUFFER_LENGTH_NORMAL, NULL, 0, out);
}
EXPORT_SYMBOL(nb04_wmi_ab_method_int_out);

int wmi_update_device_status_keyboard(struct device_keyboard_status_t *kbds)
{
	u8 arg[AB_INPUT_BUFFER_LENGTH_NORMAL] = {0};
	u8 out[AB_OUTPUT_BUFFER_LENGTH] = {0};
	u16 wmi_return;
	int result, retry_count = 3;

	if (!kbds)
		return -EINVAL;

	arg[0] = WMI_DEVICE_TYPE_ID_KEYBOARD;

	while (retry_count--) {
		result = nb04_wmi_ab_method_buffer(2, arg, out);
		if (result)
			return result;

		wmi_return = (out[1] << 8) | out[0];
		if (wmi_return != WMI_RETURN_STATUS_SUCCESS)
			return -EIO;

		if (out[3] < WMI_KEYBOARD_TYPE_MAX)
			break;

		// Sometimes initial read has proved to fail without having a fail
		// status. However the returned keyboard type in this case is invalid.
		pr_debug("Unexpected keyboard type (%d), retry read\n", out[3]);
		msleep(50);
	}

	// If, despite retries, not getting a valid keyboard type => give up
	if (out[3] >= WMI_KEYBOARD_TYPE_MAX) {
		pr_err("Failed to identifiy keyboard type\n");
		return -ENODEV;
	}

	kbds->keyboard_state_enabled = out[2] == 1;
	kbds->keyboard_type = out[3];
	kbds->keyboard_sidebar_support = out[4] == 1;
	kbds->keyboard_matrix = out[5];

	return 0;
}
EXPORT_SYMBOL(wmi_update_device_status_keyboard);

int wmi_set_whole_keyboard(u8 red, u8 green, u8 blue, int brightness)
{
	u8 arg[AB_INPUT_BUFFER_LENGTH_NORMAL] = {0};
	u64 out;
	u8 brightness_byte = 0xfe, enable_byte = 0x01;
	int result;

	if (brightness == 0)
		enable_byte = 0x00;
	else if (brightness > 0 && brightness <= 10)
		brightness_byte = brightness;

	arg[0] = 0x00;
	arg[1] = red;
	arg[2] = green;
	arg[3] = blue;
	arg[4] = brightness_byte;
	arg[5] = 0xfe;
	arg[6] = 0x00;
	arg[7] = enable_byte;

	result = nb04_wmi_ab_method_int_out(3, arg, &out);
	if (result)
		return result;

	if (out != 0)
		return -EIO;

	return 0;
}
EXPORT_SYMBOL(wmi_set_whole_keyboard);

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 3, 0)
static int tuxedo_nb04_wmi_ab_probe(struct wmi_device *wdev)
#else
static int tuxedo_nb04_wmi_ab_probe(struct wmi_device *wdev, const void *dummy_context)
#endif
{
	int err = 0;

	pr_debug("driver probe\n");

	if (!tuxedo_is_compatible())
		return -ENODEV;

	if (!wmi_has_guid(NB04_WMI_AB_GUID))
		return -ENODEV;

	mutex_lock(&nb04_wmi_ab_lock);
	if (__wmi_dev)
		err = -EBUSY;
	else
		__wmi_dev = wdev;
	mutex_unlock(&nb04_wmi_ab_lock);

	return err;
}

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
static int tuxedo_nb04_wmi_ab_remove(struct wmi_device *wdev)
#else
static void tuxedo_nb04_wmi_ab_remove(struct wmi_device *wdev)
#endif
{
	mutex_lock(&nb04_wmi_ab_lock);
	if (__wmi_dev == wdev)
		__wmi_dev = NULL;
	mutex_unlock(&nb04_wmi_ab_lock);
	pr_debug("driver remove\n");

#if LINUX_VERSION_CODE < KERNEL_VERSION(5, 13, 0)
	return 0;
#endif
}

static const struct wmi_device_id tuxedo_nb04_wmi_ab_device_ids[] = {
	{ .guid_string = NB04_WMI_AB_GUID },
	{ }
};

static struct wmi_driver tuxedo_nb04_wmi_ab_driver = {
	.driver = {
		.name = "tuxedo_nb04_wmi_ab",
		.owner = THIS_MODULE
	},
	.id_table = tuxedo_nb04_wmi_ab_device_ids,
	.probe = tuxedo_nb04_wmi_ab_probe,
	.remove = tuxedo_nb04_wmi_ab_remove,
};

module_wmi_driver(tuxedo_nb04_wmi_ab_driver);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("Driver for NB04 WMI AB methods");
MODULE_LICENSE("GPL");

MODULE_DEVICE_TABLE(wmi, tuxedo_nb04_wmi_ab_device_ids);
MODULE_ALIAS("wmi:" NB04_WMI_AB_GUID);
