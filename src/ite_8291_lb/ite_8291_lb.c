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
#include <linux/version.h>
#include <linux/module.h>
#include <linux/device.h>
#include <linux/hid.h>
#include <linux/dmi.h>
#include <linux/led-class-multicolor.h>
#include <linux/of.h>
#include <linux/delay.h>

// USB HID control data write size
#define HID_DATA_SIZE 8

#define LIGHTBAR_MAX_BRIGHTNESS		0x64
#define LIGHTBAR_DEFAULT_BRIGHTNESS	0x00
#define LIGHTBAR_DEFAULT_COLOR_RED	0xff
#define LIGHTBAR_DEFAULT_COLOR_GREEN	0xff
#define LIGHTBAR_DEFAULT_COLOR_BLUE	0xff

struct ite8291_driver_data_t {
	struct led_classdev_mc mcled_cdev_lightbar;
	struct mc_subled mcled_cdev_subleds_lightbar[3];
};

static void stop_hw(struct hid_device *hdev)
{
	hid_hw_power(hdev, PM_HINT_NORMAL);
	hid_hw_close(hdev);
	hid_hw_stop(hdev);
}

static int start_hw(struct hid_device *hdev)
{
	int result;

	result = hid_hw_start(hdev, HID_CONNECT_DEFAULT);
	if (result)
		return result;

	result = hid_hw_power(hdev, PM_HINT_FULLON);
	if (result)
		goto err_stop;

	result = hid_hw_open(hdev);
	if (result)
		goto err_power;

	return 0;

err_power:
	hid_hw_power(hdev, PM_HINT_NORMAL);
err_stop:
	hid_hw_stop(hdev);
	return result;
}


/**
 * Color scaling quirk list
 */
static void color_scaling(struct hid_device *hdev, u8 *red, u8 *green, u8 *blue)
{
#if LINUX_VERSION_CODE >= KERNEL_VERSION(4, 18, 0)
	if (dmi_match(DMI_PRODUCT_SKU, "STEPOL1XA04") && hdev->product == 0x6010) {
		*green = (100 * *green) / 255;
		*blue = (100 * *blue) / 255;
	} else if (dmi_match(DMI_PRODUCT_SKU, "STELLARIS1XI05") && hdev->product == 0x6010) {
		*green = (100 * *green) / 255;
		*blue = (100 * *blue) / 255;
	} else if (dmi_match(DMI_PRODUCT_SKU, "STELLARIS17I06") && hdev->product == 0x6010) {
		*green = (100 * *green) / 255;
		*blue = (100 * *blue) / 255;
	} else if (dmi_match(DMI_BOARD_NAME, "XxAF5xxx") ||
		   dmi_match(DMI_BOARD_NAME, "XxAF5xxx_mLED")) {
		*green = (200 * *green) / 255;
	}
#endif
}

/**
 * Write control data
 */
static int ite8291_write_control(struct hid_device *hdev, u8 *ctrl_data)
{
	int result = 0;
	u8 *buf;
	if (hdev == NULL)
		return -ENODEV;

	buf = kzalloc(HID_DATA_SIZE, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;

	memcpy(buf, ctrl_data, (size_t) HID_DATA_SIZE);

	result = hid_hw_raw_request(hdev, buf[0], buf, HID_DATA_SIZE,
				    HID_FEATURE_REPORT, HID_REQ_SET_REPORT);
	kfree(buf);

	if (result < 0)
		return result;
	return result == HID_DATA_SIZE ? 0 : -EIO;
}

/**
 * Set mono color
 * 
 * @param red Range 0x00 - 0xff
 * @param green Range 0x00 - 0xff
 * @param blue Range 0x00 - 0xff
 * @param brightness Range 0x00 - 0x64
 */
static int ite8291_write_lightbar_mono(struct hid_device *hdev, u8 red, u8 green, u8 blue, u8 brightness)
{
	int result;
	if (hdev == NULL)
		return -ENODEV;

	if (brightness > 0x64)
		return -EINVAL;

	color_scaling(hdev, &red, &green, &blue);

	switch (hdev->product) {
	case 0x6010:
		result = ite8291_write_control(hdev, (u8[]){ 0x14, 0x00, 0x01, red, green, blue, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x02, 0x01, 0x01, brightness, 0x08, 0x00, 0x00 });
		if (result)
			return result;
		break;

	case 0x7000:
		result = ite8291_write_control(hdev, (u8[]){ 0x14, 0x01, 0x01, red, green, blue, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x21, 0x01, 0x01, brightness, 0x01, 0x00, 0x00 });
		if (result)
			return result;
		break;

	case 0x7001:
		result = ite8291_write_control(hdev, (u8[]){ 0x14, 0x00, 0x01, red, green, blue, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x22, 0x01, 0x01, brightness, 0x01, 0x00, 0x00 });
		if (result)
			return result;
		break;

	default:
		return -ENOSYS;
	}

	return 0;
}

static int ite8291_write_on(struct hid_device *hdev)
{
	int result;
	switch (hdev->product) {
	case 0x6010:
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x00, 0x01, 0x04, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	case 0x7000:
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x01, 0x01, 0x04, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	case 0x7001:
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x02, 0x01, 0x04, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	default:
		return -ENOSYS;
	}

	return 0;
}

static int ite8291_write_off(struct hid_device *hdev)
{
	int result;
	switch (hdev->product) {
	case 0x6010:
		// Explicitly write mono color "off" due to issue with turning off reliably (especially for sleep)
		result = ite8291_write_lightbar_mono(hdev, 0, 0, 0, 0);
		if (result)
			return result;
		msleep(50);

		result = ite8291_write_control(hdev, (u8[]){ 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	case 0x7000:
		result = ite8291_write_control(hdev, (u8[]){ 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	case 0x7001:
		result = ite8291_write_control(hdev, (u8[]){ 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x08, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00 });
		if (result)
			return result;
		result = ite8291_write_control(hdev, (u8[]){ 0x1a, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01 });
		if (result)
			return result;
		break;

	default:
		return -ENOSYS;
	}

	return 0;
}

static int ite8291_write_state(struct hid_device *hdev)
{
	struct ite8291_driver_data_t *ite8291_driver_data = hid_get_drvdata(hdev);
	struct led_classdev_mc *mcled_cdev = &ite8291_driver_data->mcled_cdev_lightbar;

	// For now only supports mono color state saved in sysfs structure
	return ite8291_write_lightbar_mono(hdev,
					   mcled_cdev->subled_info[0].intensity,
					   mcled_cdev->subled_info[1].intensity,
					   mcled_cdev->subled_info[2].intensity,
					   ite8291_driver_data->mcled_cdev_lightbar.led_cdev.brightness);
}

static int leds_set_brightness_mc_lightbar(struct led_classdev *led_cdev, enum led_brightness brightness) {
	struct device *dev = led_cdev->dev->parent;
	struct hid_device *hdev = to_hid_device(dev);
	struct led_classdev_mc *mcled_cdev = lcdev_to_mccdev(led_cdev);

	return ite8291_write_lightbar_mono(hdev,
					 mcled_cdev->subled_info[0].intensity,
					 mcled_cdev->subled_info[1].intensity,
					 mcled_cdev->subled_info[2].intensity,
					 brightness);
}

static int ite8291_init_leds(struct hid_device *hdev)
{
	struct ite8291_driver_data_t *ite8291_driver_data = hid_get_drvdata(hdev);
	int retval;

	ite8291_driver_data->mcled_cdev_lightbar.led_cdev.name = "rgb:" "lightbar";
	ite8291_driver_data->mcled_cdev_lightbar.led_cdev.max_brightness = LIGHTBAR_MAX_BRIGHTNESS;
	ite8291_driver_data->mcled_cdev_lightbar.led_cdev.brightness_set_blocking = &leds_set_brightness_mc_lightbar;
	ite8291_driver_data->mcled_cdev_lightbar.led_cdev.brightness = LIGHTBAR_DEFAULT_BRIGHTNESS;
	ite8291_driver_data->mcled_cdev_lightbar.num_colors = 3;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info = ite8291_driver_data->mcled_cdev_subleds_lightbar;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[0].color_index = LED_COLOR_ID_RED;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[0].intensity = LIGHTBAR_DEFAULT_COLOR_RED;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[0].channel = 0;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[1].color_index = LED_COLOR_ID_GREEN;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[1].intensity = LIGHTBAR_DEFAULT_COLOR_GREEN;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[1].channel = 0;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[2].color_index = LED_COLOR_ID_BLUE;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[2].intensity = LIGHTBAR_DEFAULT_COLOR_BLUE;
	ite8291_driver_data->mcled_cdev_lightbar.subled_info[2].channel = 0;

	retval = devm_led_classdev_multicolor_register(&hdev->dev, &ite8291_driver_data->mcled_cdev_lightbar);
	if (retval != 0)
		return retval;

	return 0;
}

static int driver_probe_callb(struct hid_device *hdev, const struct hid_device_id *id)
{
	int result;
	struct ite8291_driver_data_t *ite8291_driver_data;
	bool exclude_device = false;

	// Unused devices in Stellaris Gen5 models
#if LINUX_VERSION_CODE >= KERNEL_VERSION(4, 18, 0)
	if (dmi_match(DMI_PRODUCT_SKU, "STELLARIS1XI05") &&
	    !dmi_match(DMI_PRODUCT_FAMILY, "STELLARIS17I05") &&
	    hdev->product == 0x6010)
		exclude_device = true;
#endif

	if (exclude_device) {
		pr_info("Note: device excluded, not binding to device %0#6x\n", hdev->product);
		return -ENODEV;
	}

	result = hid_parse(hdev);
	if (result) {
		pr_err("hid_parse failed\n");
		return result;
	}

	ite8291_driver_data = devm_kzalloc(&hdev->dev, sizeof(*ite8291_driver_data), GFP_KERNEL);
	if (!ite8291_driver_data)
		return -ENOMEM;

	hid_set_drvdata(hdev, ite8291_driver_data);

	result = start_hw(hdev);
	if (result)
		return result;

	result = ite8291_init_leds(hdev);
	if (result)
		goto err_stop;

	result = ite8291_write_on(hdev);
	if (result)
		goto err_led;
	result = ite8291_write_state(hdev);
	if (result)
		goto err_led;

	return 0;

err_led:
	devm_led_classdev_multicolor_unregister(&hdev->dev, &ite8291_driver_data->mcled_cdev_lightbar);
err_stop:
	stop_hw(hdev);
	return result;
}

static void driver_remove_callb(struct hid_device *hdev)
{
	struct ite8291_driver_data_t *ite8291_driver_data = hid_get_drvdata(hdev);

	devm_led_classdev_multicolor_unregister(&hdev->dev, &ite8291_driver_data->mcled_cdev_lightbar);

	ite8291_write_off(hdev);

	stop_hw(hdev);
	pr_debug("driver remove\n");
}

#ifdef CONFIG_PM
static int driver_suspend_callb(struct hid_device *hdev, pm_message_t message)
{
	pr_debug("driver suspend\n");
	return ite8291_write_off(hdev);
}

static int driver_resume_callb(struct hid_device *hdev)
{
	int result;
	pr_debug("driver resume\n");
	result = ite8291_write_on(hdev);
	if (result)
		return result;
	return ite8291_write_state(hdev);
}

static int driver_reset_resume_callb(struct hid_device *hdev)
{
	return driver_resume_callb(hdev);
}
#endif

static const struct hid_device_id ite8291_device_table[] = {
	{ HID_USB_DEVICE(0x048d, 0x6010) },
	{ HID_USB_DEVICE(0x048d, 0x7000) },
	{ HID_USB_DEVICE(0x048d, 0x7001) },
	{ }
};
MODULE_DEVICE_TABLE(hid, ite8291_device_table);

static struct hid_driver ite8291_driver = {
	.name = KBUILD_MODNAME,
	.probe = driver_probe_callb,
	.remove = driver_remove_callb,
	.id_table = ite8291_device_table,
#ifdef CONFIG_PM
	.suspend = driver_suspend_callb,
	.resume = driver_resume_callb,
	.reset_resume = driver_reset_resume_callb
#endif
};
module_hid_driver(ite8291_driver);

MODULE_AUTHOR("TUXEDO Computers GmbH <tux@tuxedocomputers.com>");
MODULE_DESCRIPTION("Driver for ITE RGB lightbars");
MODULE_LICENSE("GPL");
