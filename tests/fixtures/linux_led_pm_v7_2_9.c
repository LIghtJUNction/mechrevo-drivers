// SPDX-License-Identifier: GPL-2.0-only
/*
 * LED Class Core
 * Copyright (C) 2005 John Lenz <lenz@cs.wisc.edu>
 * Copyright (C) 2005-2007 Richard Purdie <rpurdie@openedhand.com>
 * Unmodified function excerpts from official Linux v7.2.9 led-class.c.
 * Commit: 5fce161649b4d779d1b76d9fcd52dc77779774b8
 * https://github.com/gregkh/linux/blob/5fce161649b4d779d1b76d9fcd52dc77779774b8/drivers/leds/led-class.c
 * Full original file SHA-256:
 * 50913ab981c8431a834779b4d0ad9673393629239415199818b3cc9945c44dad
 * Kernel services and hierarchy scheduling are mocked by the harness.
 */

void led_classdev_suspend(struct led_classdev *led_cdev)
{
	led_cdev->flags |= LED_SUSPENDED;
	led_set_brightness_nopm(led_cdev, 0);
	flush_work(&led_cdev->set_brightness_work);
}

void led_classdev_resume(struct led_classdev *led_cdev)
{
	led_set_brightness_nopm(led_cdev, led_cdev->brightness);

	if (led_cdev->flash_resume)
		led_cdev->flash_resume(led_cdev);

	led_cdev->flags &= ~LED_SUSPENDED;
}

static int led_suspend(struct device *dev)
{
	struct led_classdev *led_cdev = dev_get_drvdata(dev);

	if (led_cdev->flags & LED_CORE_SUSPENDRESUME)
		led_classdev_suspend(led_cdev);

	return 0;
}

static int led_resume(struct device *dev)
{
	struct led_classdev *led_cdev = dev_get_drvdata(dev);

	if (led_cdev->flags & LED_CORE_SUSPENDRESUME)
		led_classdev_resume(led_cdev);

	return 0;
}
