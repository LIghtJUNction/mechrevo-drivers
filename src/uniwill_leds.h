/* SPDX-License-Identifier: GPL-2.0+ */
/*!
 * Copyright (c) 2018-2020 TUXEDO Computers GmbH <tux@tuxedocomputers.com>
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

#ifndef UNIWILL_LEDS_H
#define UNIWILL_LEDS_H

#include <linux/platform_device.h>
#include <linux/version.h>

typedef enum {
	UNIWILL_KB_BACKLIGHT_TYPE_NONE,
	UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR,
	UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS,
	UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB,
	UNIWILL_KB_BACKLIGHT_TYPE_PER_KEY_RGB
} uniwill_kb_backlight_type_t;

int uniwill_leds_init(struct platform_device *dev);
int uniwill_leds_remove(struct platform_device *dev);

uniwill_kb_backlight_type_t uniwill_leds_get_backlight_type_extern(void);
void uniwill_leds_restore_state_extern(void);
bool uniwill_leds_notify_brightness_change_extern(void);

// TODO The following should go into a seperate .c file, but for this to work more reworking is required in the tuxedo_keyboard structure.

//#include "uniwill_leds.h"

#include <linux/types.h>
#include <linux/leds.h>
#include <linux/led-class-multicolor.h>
#include <linux/mutex.h>
#include "uniwill_interfaces.h"

#define UNIWILL_KBD_BRIGHTNESS_MAX_WHITE		0x02
#define UNIWILL_KBD_BRIGHTNESS_DEFAULT_WHITE		0x00

#define UNIWILL_KBD_BRIGHTNESS_MAX_WHITE_5		0x04

#define UNIWILL_KBD_BRIGHTNESS_MAX_1_ZONE_RGB		0x04
#define UNIWILL_KBD_BRIGHTNESS_DEFAULT_1_ZONE_RGB	0x00

#define UNIWILL_KBD_INTENSITY_MAX_1_ZONE_RGB		0xff
#define UNIWILL_KBD_COLOR_DEFAULT_RED			0xff
#define UNIWILL_KBD_COLOR_DEFAULT_GREEN			0xff
#define UNIWILL_KBD_COLOR_DEFAULT_BLUE			0xff

static uniwill_kb_backlight_type_t uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_NONE;
static u8 uniwill_barebone_id = 0;
static bool uniwill_kbl_brightness_ec_controlled = false;
static bool uw_leds_initialized = false;
/* Serialize the driver's multi-register LED operations. LED core owns the
 * requested brightness/intensity fields; this does not lock its writers.
 * Lock order: fan command/state -> LED -> EC transaction -> transport.
 * Never drain LED work or unregister a classdev while this mutex is held. */
static DEFINE_MUTEX(uniwill_led_lock);

static int uniwill_write_kbd_bl_brightness(u8 brightness)
{
	/* Preserve the existing low nibble, replace brightness and the apply bit. */
	return uniwill_update_ec_ram(UW_EC_REG_KBD_BL_STATUS, 0xf0,
				    (brightness << 5) | UW_EC_REG_KBD_BL_STATUS_SUBCMD_RESET);
}

static int uniwill_write_kbd_bl_brightness_white_workaround(u8 brightness)
{
	int result = 0;
	u8 data = 0;

	// When keyboard backlight is off, new settings to 0x078c do not get applied automatically
	// on Pulse Gen1/2 until next keypress or manual change to the 0x1808 immediate brightness
	// value for some reason.
	// Sidenote: IBP Gen6/7 has immediate brightness value on 0x1802 and not on 0x1808, but does
	// not need this workaround. No model check required because this doesn't do anything on
	// these devices.

	result = uniwill_read_ec_ram(UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, &data);
	if (result) {
		pr_debug("uniwill_write_kbd_bl_white(): Get UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE failed.\n");
		return result;
	}

	if (!data && brightness) {
		result = uniwill_write_ec_ram(UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE, 0x01);
		if (result) {
			pr_debug("uniwill_write_kbd_bl_white(): Set UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS_IMMEDIATE failed.\n");
			return result;
		}
	}

	return uniwill_write_kbd_bl_brightness(brightness);
}

// Converts the range 0-255 to the range 0-50
static int tf_convert_rgb_range(u8 input) {
	return input*200/(255*4);
}

static int uniwill_write_kbd_bl_color(u8 red, u8 green, u8 blue)
{
	int result = 0;

	// If, after conversion, all three (red, green, and blue) values are zero at the same time,
	// a special case is triggered in the EC and (probably device dependent) default values are
	// written instead.

	result = uniwill_write_ec_ram(UW_EC_REG_KBD_BL_RGB_RED_BRIGHTNESS, tf_convert_rgb_range(red));
	if (result)
		return result;

	result = uniwill_write_ec_ram(UW_EC_REG_KBD_BL_RGB_GREEN_BRIGHTNESS, tf_convert_rgb_range(green));
	if (result)
		return result;

	result = uniwill_write_ec_ram(UW_EC_REG_KBD_BL_RGB_BLUE_BRIGHTNESS, tf_convert_rgb_range(blue));
	if (result)
		return result;

	/* USB powershare uses another bit in this same EC byte. */
	result = uniwill_update_ec_ram(UW_EC_REG_KBD_BL_RGB_MODE, 0,
				      UW_EC_REG_KBD_BL_RGB_MODE_BIT_APPLY_COLOR);
	if (result)
		return result;

	pr_debug("Wrote kbd color [%0#4x, %0#4x, %0#4x]\n", red, green, blue);

	return result;
}

static int uniwill_leds_set_brightness(struct led_classdev *led_cdev __always_unused, enum led_brightness brightness) {
	int result = 0;

	mutex_lock(&uniwill_led_lock);
	result = uniwill_write_kbd_bl_brightness_white_workaround(brightness);
	if (result) {
		pr_debug("uniwill_leds_set_brightness(): uniwill_write_kbd_bl_white() failed\n");
	}
	mutex_unlock(&uniwill_led_lock);
	return result;
}

static int uniwill_leds_set_brightness_mc(struct led_classdev *led_cdev, enum led_brightness brightness) {
	int result = 0;
	struct led_classdev_mc *mcled_cdev = lcdev_to_mccdev(led_cdev);

	mutex_lock(&uniwill_led_lock);
	if (mcled_cdev->subled_info[0].intensity == 0 &&
	    mcled_cdev->subled_info[1].intensity == 0 &&
	    mcled_cdev->subled_info[2].intensity == 0) {
		pr_debug("uniwill_leds_set_brightness_mc(): Trigger RGB 0x000000 special case\n");
		result = uniwill_write_kbd_bl_brightness(0);
		if (result) {
			pr_debug("uniwill_leds_set_brightness_mc(): uniwill_write_kbd_bl_brightness() failed\n");
			goto unlock;
		}
	}
	else {
		result = uniwill_write_kbd_bl_color(mcled_cdev->subled_info[0].intensity,
					       mcled_cdev->subled_info[1].intensity,
					       mcled_cdev->subled_info[2].intensity);
		if (result) {
			pr_debug("uniwill_leds_set_brightness_mc(): uniwill_write_kbd_bl_rgb() failed\n");
			goto unlock;
		}

		result = uniwill_write_kbd_bl_brightness(brightness);
		if (result) {
			pr_debug("uniwill_leds_set_brightness_mc(): uniwill_write_kbd_bl_brightness() failed\n");
			goto unlock;
		}
	}

unlock:
	mutex_unlock(&uniwill_led_lock);
	return result;
}

static struct led_classdev uniwill_led_cdev = {
	.name = "white:" LED_FUNCTION_KBD_BACKLIGHT,
	.max_brightness = UNIWILL_KBD_BRIGHTNESS_MAX_WHITE,
	.flags = LED_CORE_SUSPENDRESUME,
	.brightness_set_blocking = &uniwill_leds_set_brightness,
	.brightness = UNIWILL_KBD_BRIGHTNESS_DEFAULT_WHITE,
};

static struct mc_subled uw_mcled_cdev_subleds[3] = {
	{
		.color_index = LED_COLOR_ID_RED,
		.intensity = UNIWILL_KBD_COLOR_DEFAULT_RED,
#if LINUX_VERSION_CODE >= KERNEL_VERSION(7, 2, 0)
		.max_intensity = UNIWILL_KBD_INTENSITY_MAX_1_ZONE_RGB,
#endif
		.channel = 0
	},
	{
		.color_index = LED_COLOR_ID_GREEN,
		.intensity = UNIWILL_KBD_COLOR_DEFAULT_GREEN,
#if LINUX_VERSION_CODE >= KERNEL_VERSION(7, 2, 0)
		.max_intensity = UNIWILL_KBD_INTENSITY_MAX_1_ZONE_RGB,
#endif
		.channel = 0
	},
	{
		.color_index = LED_COLOR_ID_BLUE,
		.intensity = UNIWILL_KBD_COLOR_DEFAULT_BLUE,
#if LINUX_VERSION_CODE >= KERNEL_VERSION(7, 2, 0)
		.max_intensity = UNIWILL_KBD_INTENSITY_MAX_1_ZONE_RGB,
#endif
		.channel = 0
	}
};

static struct led_classdev_mc uniwill_mcled_cdev = {
	.led_cdev.name = "rgb:" LED_FUNCTION_KBD_BACKLIGHT,
	.led_cdev.max_brightness = UNIWILL_KBD_BRIGHTNESS_MAX_1_ZONE_RGB,
	.led_cdev.flags = LED_CORE_SUSPENDRESUME,
	.led_cdev.brightness_set_blocking = &uniwill_leds_set_brightness_mc,
	.led_cdev.brightness = UNIWILL_KBD_BRIGHTNESS_DEFAULT_1_ZONE_RGB,
	.num_colors = 3,
	.subled_info = uw_mcled_cdev_subleds
};

static const struct dmi_system_id force_no_ec_led_control[] = {
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_PRODUCT_SKU, "STELLARIS1XA05"),
		},
	},
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_PRODUCT_SKU, "STELLSL15I06"),
		},
	},
	{ }
};

static const struct dmi_system_id kbl_type_fixed_color_5_levels[] = {
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_BOARD_NAME, "GXxHRXx"),
		},
	},
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_BOARD_NAME, "GXxMRXx"),
		},
	},
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_BOARD_NAME, "XxHP4NAx"),
		},
	},
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_BOARD_NAME, "XxKK4NAx_XxSP4NAx"),
		},
	},
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "TUXEDO"),
			DMI_MATCH(DMI_BOARD_NAME, "XxAR4NAx"),
		},
	},
	{ }
};

int uniwill_leds_init(struct platform_device *dev)
{
	int result = 0, i = 0;
	u8 data = 0;
	uniwill_kb_backlight_type_t backlight_type;
	bool ec_controlled;

	mutex_lock(&uniwill_led_lock);
	uw_leds_initialized = false;
	uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_NONE;
	uniwill_kbl_brightness_ec_controlled = false;

	for (i = 0; i < 3; ++i) {
		if (i) {
			pr_err("Reading barebone ID failed. Retrying ...\n");
		}

		result = uniwill_read_ec_ram(UW_EC_REG_BAREBONE_ID, &uniwill_barebone_id);
		if (!result && uniwill_barebone_id) {
			break;
		}
		msleep(200);
	}
	if (result || !uniwill_barebone_id) {
		pr_err("Reading barebone ID failed.\n");
		result = result ? result : -EIO;
		goto unlock;
	}
	pr_debug("EC Barebone ID: %#04x\n", uniwill_barebone_id);

	// Some devices need this bit set for the keyboard backlight
	// to be controllable by the driver. On some devices, it's
	// set by default, but on some it's not enabled under certain
	// conditions, so we make sure it's always set.
	// Known affected devices:
	// - IBP Gen7-9
	// - Polaris Gen4-5
	result = uniwill_read_ec_ram(UW_EC_REG_FEATURES_1, &data);
	if (result) {
		pr_err("Reading keyboard backlight features failed: %d\n", result);
		goto unlock;
	}
	data |= UW_EC_REG_FEATURES_1_BIT_FIXED_COLOR_5_ENABLE;
	result = uniwill_write_ec_ram(UW_EC_REG_FEATURES_1, data);
	if (result) {
		pr_err("Enabling keyboard backlight control failed: %d\n", result);
		goto unlock;
	}

	if (dmi_check_system(force_no_ec_led_control)) {
		pr_debug("Skip uniwill_kb_backlight_type checks because of quirk.\n");
	}
	else if (uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PFxxxxx ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PFxMxxx ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH4TRX1 ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH4TUX1 ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH4TQx1 ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH6TRX1 ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH6TQxx ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH4Axxx ||
		 uniwill_barebone_id == UW_EC_REG_BAREBONE_ID_VALUE_PH4Pxxx) {
		uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR;
		uniwill_kbl_brightness_ec_controlled = true;
	}
	else if (dmi_check_system(kbl_type_fixed_color_5_levels)) {
		uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS;
		uniwill_kbl_brightness_ec_controlled = true;
	}
	else {
		result = uniwill_read_ec_ram(UW_EC_REG_FEATURES_1, &data);
		if (result) {
			pr_err("Reading features 1 failed.\n");
			goto unlock;
		}
		pr_debug("UW_EC_REG_FEATURES_1: 0x%02x\n", data);

		if (data & UW_EC_REG_FEATURES_1_BIT_1_ZONE_RGB_KB) {
			uniwill_kb_backlight_type = UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB;
			uniwill_kbl_brightness_ec_controlled = true;
		}
	}
	pr_debug("Keyboard backlight type: 0x%02x\n", uniwill_kb_backlight_type);
	backlight_type = uniwill_kb_backlight_type;
	ec_controlled = uniwill_kbl_brightness_ec_controlled;
	/* Registration may invoke LED core code. Publish restore/notification
	 * availability only after it has succeeded, without holding our mutex. */
	mutex_unlock(&uniwill_led_lock);

	if (backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR) {
		pr_debug("Registering fixed color leds interface\n");
		uniwill_led_cdev.max_brightness = UNIWILL_KBD_BRIGHTNESS_MAX_WHITE;
		uniwill_led_cdev.flags = LED_CORE_SUSPENDRESUME;
		if (ec_controlled)
			uniwill_led_cdev.flags |= LED_BRIGHT_HW_CHANGED;
		result = led_classdev_register(&dev->dev, &uniwill_led_cdev);
		if (result) {
			pr_err("Registering fixed color leds interface failed\n");
			return result;
		}
	}
	else if (backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS) {
		pr_debug("Registering fixed color 5 levels leds interface\n");
		uniwill_led_cdev.max_brightness = UNIWILL_KBD_BRIGHTNESS_MAX_WHITE_5;
		uniwill_led_cdev.flags = LED_CORE_SUSPENDRESUME;
		if (ec_controlled)
			uniwill_led_cdev.flags |= LED_BRIGHT_HW_CHANGED;
		result = led_classdev_register(&dev->dev, &uniwill_led_cdev);
		if (result) {
			pr_err("Registering fixed color 5 levels leds interface failed\n");
			return result;
		}
	}
	else if (backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB) {
		pr_debug("Registering single zone rgb leds interface\n");
		uniwill_mcled_cdev.led_cdev.flags = LED_CORE_SUSPENDRESUME;
		if (ec_controlled)
			uniwill_mcled_cdev.led_cdev.flags |= LED_BRIGHT_HW_CHANGED;
		result = devm_led_classdev_multicolor_register(&dev->dev, &uniwill_mcled_cdev);
		if (result) {
			pr_err("Registering single zone rgb leds interface failed\n");
			return result;
		}
	}

	mutex_lock(&uniwill_led_lock);
	uw_leds_initialized = true;
unlock:
	mutex_unlock(&uniwill_led_lock);
	return result;
}
EXPORT_SYMBOL(uniwill_leds_init);

int uniwill_leds_remove(struct platform_device *dev)
{
	uniwill_kb_backlight_type_t backlight_type;
	bool initialized;

	mutex_lock(&uniwill_led_lock);
	initialized = uw_leds_initialized;
	uw_leds_initialized = false;
	backlight_type = uniwill_kb_backlight_type;
	mutex_unlock(&uniwill_led_lock);
	/* Unregister queues OFF and drains our callback, which takes this lock. */
	if (initialized) {

		if (backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR ||
		    backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS) {
			led_classdev_unregister(&uniwill_led_cdev);
		}
		else if (backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB) {
			devm_led_classdev_multicolor_unregister(&dev->dev, &uniwill_mcled_cdev);
		}
	}

	return 0;
}
EXPORT_SYMBOL(uniwill_leds_remove);

uniwill_kb_backlight_type_t uniwill_leds_get_backlight_type_extern(void) {
	uniwill_kb_backlight_type_t backlight_type;
	mutex_lock(&uniwill_led_lock);
	backlight_type = uniwill_kb_backlight_type;
	mutex_unlock(&uniwill_led_lock);
	return backlight_type;
}
EXPORT_SYMBOL(uniwill_leds_get_backlight_type_extern);

void uniwill_leds_restore_state_extern(void) {
	mutex_lock(&uniwill_led_lock);
	if (!uw_leds_initialized)
		goto unlock;

	if (uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR ||
	    uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS) {
		if (uniwill_write_kbd_bl_brightness_white_workaround(uniwill_led_cdev.brightness)) {
			pr_debug("uniwill_leds_restore_state_extern(): uniwill_write_kbd_bl_white() failed\n");
		}
	}
	else if (uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB) {
		/* Keep the same all-black workaround as the brightness callback:
		 * writing three zero channels can select an EC default color. */
		if (uniwill_mcled_cdev.subled_info[0].intensity == 0 &&
		    uniwill_mcled_cdev.subled_info[1].intensity == 0 &&
		    uniwill_mcled_cdev.subled_info[2].intensity == 0) {
			if (uniwill_write_kbd_bl_brightness(0))
				pr_debug("uniwill_leds_restore_state_extern(): restoring RGB black failed\n");
			goto unlock;
		}
		if (uniwill_write_kbd_bl_color(uniwill_mcled_cdev.subled_info[0].intensity,
					     uniwill_mcled_cdev.subled_info[1].intensity,
					     uniwill_mcled_cdev.subled_info[2].intensity)) {
			pr_debug("uniwill_leds_restore_state_extern(): uniwill_write_kbd_bl_rgb() failed\n");
		}
		if (uniwill_write_kbd_bl_brightness(uniwill_mcled_cdev.led_cdev.brightness)) {
			pr_debug("uniwill_leds_restore_state_extern(): uniwill_write_kbd_bl_brightness() failed\n");
		}
	}
unlock:
	mutex_unlock(&uniwill_led_lock);
}
EXPORT_SYMBOL(uniwill_leds_restore_state_extern);

bool uniwill_leds_notify_brightness_change_extern(void) {
	int result = 0;
	u8 data = 0, brightness = 0;
	struct led_classdev *led_cdev;
	bool handled = false;

	mutex_lock(&uniwill_led_lock);
	if (!uw_leds_initialized || !uniwill_kbl_brightness_ec_controlled)
		goto unlock;
	if (uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR ||
	    uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_FIXED_COLOR_5_LEVELS)
		led_cdev = &uniwill_led_cdev;
	else if (uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB)
		led_cdev = &uniwill_mcled_cdev.led_cdev;
	else
		goto unlock;
	handled = true;
	/* LED core's temporary suspend OFF must not replace its desired value. */
	if (READ_ONCE(led_cdev->flags) & LED_SUSPENDED)
		goto unlock;
	result = uniwill_read_ec_ram(UW_EC_REG_KBD_BL_STATUS, &data);
	if (result) {
		pr_debug("Reading changed keyboard backlight brightness failed: %d\n", result);
		goto unlock;
	}
	if (READ_ONCE(led_cdev->flags) & LED_SUSPENDED)
		goto unlock;
	brightness = (data >> 5) & 0x07;
	if (uniwill_kb_backlight_type == UNIWILL_KB_BACKLIGHT_TYPE_1_ZONE_RGB &&
	    led_cdev->brightness == brightness) {
		// Workaround for devices where EC does not react to FN+space in manual mode (known device: Polaris Gen2)
		result = uniwill_write_kbd_bl_brightness((brightness + 1) % 5);
		if (result) {
			pr_debug("Applying changed keyboard backlight brightness failed: %d\n", result);
			goto unlock;
		}
		brightness = (brightness + 1) % 5;
	}
	if (READ_ONCE(led_cdev->flags) & LED_SUSPENDED)
		goto unlock;
	led_cdev->brightness = brightness;
	led_classdev_notify_brightness_hw_changed(led_cdev, brightness);
unlock:
	mutex_unlock(&uniwill_led_lock);
	return handled;
}

MODULE_LICENSE("GPL");

#endif // UNIWILL_LEDS_H
