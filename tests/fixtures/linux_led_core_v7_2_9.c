// SPDX-License-Identifier: GPL-2.0-only
/*
 * LED Class Core
 * Copyright 2005-2006 Openedhand Ltd.
 * Author: Richard Purdie <rpurdie@openedhand.com>
 *
 * Unmodified function excerpts from official Linux v7.2.9 led-core.c.
 * Commit: 5fce161649b4d779d1b76d9fcd52dc77779774b8
 * https://github.com/gregkh/linux/blob/5fce161649b4d779d1b76d9fcd52dc77779774b8/drivers/leds/led-core.c
 * Full original file SHA-256:
 * 97dffa18ffcc714ad0892d4e049ea5dd4a2f3df3c6203ee227f1177237db6946
 * Kernel structures and workqueue/timer/EC services are mocked by the harness.
 */

static int __led_set_brightness(struct led_classdev *led_cdev, unsigned int value)
{
	if (!led_cdev->brightness_set)
		return -ENOTSUPP;

	led_cdev->brightness_set(led_cdev, value);

	return 0;
}

static int __led_set_brightness_blocking(struct led_classdev *led_cdev, unsigned int value)
{
	if (!led_cdev->brightness_set_blocking)
		return -ENOTSUPP;

	return led_cdev->brightness_set_blocking(led_cdev, value);
}

static void led_timer_function(struct timer_list *t)
{
	struct led_classdev *led_cdev = timer_container_of(led_cdev, t,
							   blink_timer);
	unsigned long brightness;
	unsigned long delay;

	if (!led_cdev->blink_delay_on || !led_cdev->blink_delay_off) {
		led_set_brightness_nosleep(led_cdev, LED_OFF);
		clear_bit(LED_BLINK_SW, &led_cdev->work_flags);
		return;
	}

	if (test_and_clear_bit(LED_BLINK_ONESHOT_STOP,
			       &led_cdev->work_flags)) {
		clear_bit(LED_BLINK_SW, &led_cdev->work_flags);
		return;
	}

	brightness = led_get_brightness(led_cdev);
	if (!brightness) {
		/* Time to switch the LED on. */
		if (test_and_clear_bit(LED_BLINK_BRIGHTNESS_CHANGE,
				       &led_cdev->work_flags))
			brightness = led_cdev->new_blink_brightness;
		else
			brightness = led_cdev->blink_brightness;
		delay = led_cdev->blink_delay_on;
	} else {
		/* Store the current brightness value to be able
		 * to restore it when the delay_off period is over.
		 */
		led_cdev->blink_brightness = brightness;
		brightness = LED_OFF;
		delay = led_cdev->blink_delay_off;
	}

	led_set_brightness_nosleep(led_cdev, brightness);

	/* Return in next iteration if led is in one-shot mode and we are in
	 * the final blink state so that the led is toggled each delay_on +
	 * delay_off milliseconds in worst case.
	 */
	if (test_bit(LED_BLINK_ONESHOT, &led_cdev->work_flags)) {
		if (test_bit(LED_BLINK_INVERT, &led_cdev->work_flags)) {
			if (brightness)
				set_bit(LED_BLINK_ONESHOT_STOP,
					&led_cdev->work_flags);
		} else {
			if (!brightness)
				set_bit(LED_BLINK_ONESHOT_STOP,
					&led_cdev->work_flags);
		}
	}

	mod_timer(&led_cdev->blink_timer, jiffies + msecs_to_jiffies(delay));
}

static void set_brightness_delayed_set_brightness(struct led_classdev *led_cdev,
						  unsigned int value)
{
	int ret;

	ret = __led_set_brightness(led_cdev, value);
	if (ret == -ENOTSUPP) {
		ret = __led_set_brightness_blocking(led_cdev, value);
		if (ret == -ENOTSUPP)
			/* No back-end support to set a fixed brightness value */
			return;
	}

	/* LED HW might have been unplugged, therefore don't warn */
	if (ret == -ENODEV && led_cdev->flags & LED_UNREGISTERING &&
	    led_cdev->flags & LED_HW_PLUGGABLE)
		return;

	if (ret < 0)
		dev_err(led_cdev->dev,
			"Setting an LED's brightness failed (%d)\n", ret);
}

static void set_brightness_delayed(struct work_struct *ws)
{
	struct led_classdev *led_cdev =
		container_of(ws, struct led_classdev, set_brightness_work);

	if (test_and_clear_bit(LED_BLINK_DISABLE, &led_cdev->work_flags)) {
		led_stop_software_blink(led_cdev);
		set_bit(LED_SET_BRIGHTNESS_OFF, &led_cdev->work_flags);
	}

	/*
	 * Triggers may call led_set_brightness(LED_OFF),
	 * led_set_brightness(LED_FULL) in quick succession to disable blinking
	 * and turn the LED on. Both actions may have been scheduled to run
	 * before this work item runs once. To make sure this works properly
	 * handle LED_SET_BRIGHTNESS_OFF first.
	 */
	if (test_and_clear_bit(LED_SET_BRIGHTNESS_OFF, &led_cdev->work_flags)) {
		set_brightness_delayed_set_brightness(led_cdev, LED_OFF);
		/*
		 * The consecutives led_set_brightness(LED_OFF),
		 * led_set_brightness(LED_FULL) could have been executed out of
		 * order (LED_FULL first), if the work_flags has been set
		 * between LED_SET_BRIGHTNESS_OFF and LED_SET_BRIGHTNESS of this
		 * work. To avoid ending with the LED turned off, turn the LED
		 * on again.
		 */
		if (led_cdev->delayed_set_value != LED_OFF)
			set_bit(LED_SET_BRIGHTNESS, &led_cdev->work_flags);
	}

	if (test_and_clear_bit(LED_SET_BRIGHTNESS, &led_cdev->work_flags))
		set_brightness_delayed_set_brightness(led_cdev, led_cdev->delayed_set_value);

	if (test_and_clear_bit(LED_SET_BLINK, &led_cdev->work_flags)) {
		unsigned long delay_on = led_cdev->delayed_delay_on;
		unsigned long delay_off = led_cdev->delayed_delay_off;

		led_blink_set(led_cdev, &delay_on, &delay_off);
	}
}

void led_stop_software_blink(struct led_classdev *led_cdev)
{
	timer_delete_sync(&led_cdev->blink_timer);
	led_cdev->blink_delay_on = 0;
	led_cdev->blink_delay_off = 0;
	clear_bit(LED_BLINK_SW, &led_cdev->work_flags);
}

void led_set_brightness_nopm(struct led_classdev *led_cdev, unsigned int value)
{
	/* Use brightness_set op if available, it is guaranteed not to sleep */
	if (!__led_set_brightness(led_cdev, value))
		return;

	/*
	 * Brightness setting can sleep, delegate it to a work queue task.
	 * value 0 / LED_OFF is special, since it also disables hw-blinking
	 * (sw-blink disable is handled in led_set_brightness()).
	 * To avoid a hw-blink-disable getting lost when a second brightness
	 * change is done immediately afterwards (before the work runs),
	 * it uses a separate work_flag.
	 */
	led_cdev->delayed_set_value = value;
	/* Ensure delayed_set_value is seen before work_flags modification */
	smp_mb__before_atomic();

	if (value) {
		set_bit(LED_SET_BRIGHTNESS, &led_cdev->work_flags);
	} else {
		clear_bit(LED_SET_BRIGHTNESS, &led_cdev->work_flags);
		clear_bit(LED_SET_BLINK, &led_cdev->work_flags);
		set_bit(LED_SET_BRIGHTNESS_OFF, &led_cdev->work_flags);
	}

	queue_work(led_cdev->wq, &led_cdev->set_brightness_work);
}

void led_set_brightness_nosleep(struct led_classdev *led_cdev, unsigned int value)
{
	led_cdev->brightness = min(value, led_cdev->max_brightness);

	if (led_cdev->flags & LED_SUSPENDED)
		return;

	led_set_brightness_nopm(led_cdev, led_cdev->brightness);
}

int led_set_brightness_sync(struct led_classdev *led_cdev, unsigned int value)
{
	if (led_cdev->blink_delay_on || led_cdev->blink_delay_off)
		return -EBUSY;

	led_cdev->brightness = min(value, led_cdev->max_brightness);

	if (led_cdev->flags & LED_SUSPENDED)
		return 0;

	return __led_set_brightness_blocking(led_cdev, led_cdev->brightness);
}
