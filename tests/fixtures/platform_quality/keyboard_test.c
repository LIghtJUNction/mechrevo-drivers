
static int key_reports, brightness_reports, last_brightness, rewrites;
static bool profile_changed_by_driver(void) { return false; }
static void rewrite_last_profile(void) { ++rewrites; }
static void report_gauge_key_combo(struct input_dev *input) { ++key_reports; }
static void sparse_keymap_report_known_event(struct input_dev *input, unsigned event,
                                             unsigned value, bool autorelease)
{
    ++key_reports;
}
static void nb05_leds_notify_brightness_change_extern(u8 value)
{
    ++brightness_reports;
    last_brightness = value;
}

@FUNCTIONS@

int main(void)
{
    struct driver_data_t data = {0};
    struct wmi_device dev = { .dev.drvdata = &data };
    union acpi_object object = { .buffer.type = ACPI_TYPE_BUFFER,
                                 .buffer.length = 4, .buffer.pointer = NULL };
    @NOTIFY@(&dev, &object);
    @NOTIFY@(&dev, NULL);
    assert(!key_reports && !brightness_reports);
    u8 payload[] = { 0, 3, 2, 1 };
    object.buffer.pointer = payload;
    object.buffer.length = 1;
    @NOTIFY@(&dev, &object);
    assert(!key_reports && !brightness_reports);
    object.buffer.length = 4;
    @NOTIFY@(&dev, &object);
#if @NB05@
    assert(key_reports == 2 && brightness_reports == 0); /* 0x0102 must not narrow to 2. */
    payload[3] = 0;
    @NOTIFY@(&dev, &object);
    assert(brightness_reports == 1 && last_brightness == 2);
#else
    assert(key_reports == 1);
#endif
    return 0;
}
