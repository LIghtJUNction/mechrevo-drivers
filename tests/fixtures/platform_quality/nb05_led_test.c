
static int registration_error, registrations, unregistrations, notifications, writes;
static int last_register, last_value;
static const struct dmi_system_id known_device = { .ident = IFLX14I01 };
static const struct dmi_system_id *matched_device = &known_device;
static const struct dmi_system_id *nb05_match_device(void) { return matched_device; }
static void nb05_write_ec_ram(u16 address, u8 value)
{
    ++writes;
    last_register = address;
    last_value = value;
}
static int led_classdev_register(struct device *dev, struct led_classdev *led)
{
    ++registrations;
    assert(!led->brightness_set && led->brightness_set_blocking);
    return registration_error;
}
static void led_classdev_unregister(struct led_classdev *led)
{
    assert(!__nb05_kbd_led_cdev);
    ++unregistrations;
}
static void led_classdev_notify_brightness_hw_changed(struct led_classdev *led, u8 value)
{
    assert(led == __nb05_kbd_led_cdev && led->brightness == value);
    ++notifications;
}

@FUNCTIONS@

int main(void)
{
    struct platform_device pdev = {0};
    registration_error = -EIO;
    assert(tuxedo_nb05_kbd_backlight_probe(&pdev) == -EIO);
    assert(registrations == 1 && !__nb05_kbd_led_cdev);
    free(pdev.dev.drvdata);
    registration_error = 0;
    assert(tuxedo_nb05_kbd_backlight_probe(&pdev) == 0);
    assert(registrations == 2 && __nb05_kbd_led_cdev);
    assert(nb05_leds_set_brightness(__nb05_kbd_led_cdev, 2) == 0);
    assert(writes == 1 && last_register == 0x03e2 && last_value == 0xb8);
    assert(nb05_leds_set_brightness(__nb05_kbd_led_cdev, 3) == -EINVAL);
    assert(nb05_leds_set_brightness(__nb05_kbd_led_cdev, -1) == -EINVAL);
    assert(writes == 1);
    matched_device = NULL;
    assert(nb05_leds_set_brightness(__nb05_kbd_led_cdev, 1) == -ENODEV);
    assert(writes == 1);
    nb05_leds_notify_brightness_change_extern(2);
    nb05_leds_notify_brightness_change_extern(3);
    assert(notifications == 1 && __nb05_kbd_led_cdev->brightness == 2);
    tuxedo_nb05_kbd_backlight_remove(&pdev);
    assert(unregistrations == 1 && !__nb05_kbd_led_cdev);
    nb05_leds_notify_brightness_change_extern(1);
    assert(notifications == 1);
    free(pdev.dev.drvdata);
    allocation_fails = true;
    assert(tuxedo_nb05_kbd_backlight_probe(&pdev) == -ENOMEM);
    assert(registrations == 2);
    return 0;
}
