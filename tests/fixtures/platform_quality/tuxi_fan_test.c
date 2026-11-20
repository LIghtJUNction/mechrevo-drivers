
static int fw_error, read_calls, write_calls, written_index;
static u8 speed = 173, written_speed;
static enum tuxi_fan_mode fw_mode = MANUAL, written_mode;
static int tuxi_get_fan_speed(u8 index, u8 *value)
{
    ++read_calls;
    if (fw_error)
        return fw_error;
    *value = speed;
    return 0;
}
static int tuxi_set_fan_speed(u8 index, u8 value)
{
    ++write_calls;
    written_index = index;
    written_speed = value;
    return fw_error;
}
static int tuxi_get_fan_mode(enum tuxi_fan_mode *mode)
{
    ++read_calls;
    if (fw_error)
        return fw_error;
    *mode = fw_mode;
    return 0;
}
static int tuxi_set_fan_mode(enum tuxi_fan_mode mode)
{
    ++write_calls;
    written_mode = mode;
    return fw_error;
}
static int tuxi_get_fan_temp(u8 index, u16 *value)
{
    ++read_calls;
    if (fw_error)
        return fw_error;
    *value = 3000;
    return 0;
}
static int tuxi_get_fan_rpm(u8 index, u16 *value)
{
    ++read_calls;
    if (fw_error)
        return fw_error;
    *value = 1700;
    return 0;
}

@FUNCTIONS@

int main(void)
{
    char buffer[4096];
    struct driver_data_t data = { .nr_fans = 1 };
    struct device dev = { .drvdata = &data };
    dev.kobj.dev = &dev;
    struct attribute attr = { .mode = 0644 };
    long value = 999;
    const char *label = NULL;
    ssize_t (*show[])(struct device *, struct device_attribute *, char *) = {
        fan1_pwm_show, fan2_pwm_show
    };
    ssize_t (*enable_show[])(struct device *, struct device_attribute *, char *) = {
        fan1_pwm_enable_show, fan2_pwm_enable_show
    };
    ssize_t (*enable_store[])(struct device *, struct device_attribute *, const char *, size_t) = {
        fan1_pwm_enable_store, fan2_pwm_enable_store
    };
    for (unsigned i = 0; i < ARRAY_SIZE(show); ++i) {
        assert(show[i](&dev, NULL, buffer) == 4 && !strcmp(buffer, "173\n"));
        strcpy(buffer, "unchanged");
        fw_error = -ENODEV;
        assert(show[i](&dev, NULL, buffer) == -ENODEV);
        assert(!strcmp(buffer, "unchanged"));
        assert(enable_show[i](&dev, NULL, buffer) == -ENODEV);
        assert(!strcmp(buffer, "unchanged"));
        fw_error = 0;
        assert(enable_show[i](&dev, NULL, buffer) == 2 && !strcmp(buffer, "1\n"));
        int before = write_calls;
        assert(enable_store[i](&dev, NULL, "0", 1) == -EINVAL);
        assert(enable_store[i](&dev, NULL, "3", 1) == -EINVAL);
        assert(enable_store[i](&dev, NULL, "256", 3) == -EINVAL);
        assert(write_calls == before);
        assert(enable_store[i](&dev, NULL, "1\n", 2) == 2 && written_mode == MANUAL);
        assert(enable_store[i](&dev, NULL, "2\n", 2) == 2 && written_mode == AUTO);
        fw_error = -EIO;
        assert(enable_store[i](&dev, NULL, "2", 1) == -EIO);
        fw_error = 0;
    }
    assert(fan1_pwm_store(&dev, NULL, "15", 2) == 2 && written_speed == 0);
    assert(fan1_pwm_store(&dev, NULL, "32", 2) == 2 && written_speed == 63);
    assert(fan2_pwm_store(&dev, NULL, "255", 3) == 3);
    assert(written_index == 1 && written_speed == 255);

    assert(fan_control_is_visible(&dev.kobj, &attr, 0) == 0644);
    assert(fan_control_is_visible(&dev.kobj, &attr, 2) == 0);
    assert(hwm_is_visible(&data, hwmon_fan, hwmon_fan_input, 0) == 0444);
    assert(hwm_is_visible(&data, hwmon_fan, hwmon_fan_input, 1) == 0);
    int before = read_calls;
    assert(hwm_read(&dev, hwmon_fan, hwmon_fan_input, -1, &value) == -EOPNOTSUPP);
    assert(hwm_read(&dev, hwmon_fan, hwmon_fan_input, 1, &value) == -EOPNOTSUPP);
    assert(read_calls == before && value == 999);
    assert(hwm_read_string(&dev, hwmon_fan, hwmon_fan_label, 1, &label) == -EOPNOTSUPP);
    assert(label == NULL);
    data.nr_fans = 2;
    assert(hwm_read(&dev, hwmon_temp, hwmon_temp_input, 1, &value) == 0 && value == 27000);
    assert(hwm_read(&dev, hwmon_fan, hwmon_fan_input, 1, &value) == 0 && value == 1700);
    assert(hwm_read_string(&dev, hwmon_fan, hwmon_fan_label, 1, &label) == 0);
    assert(!strcmp(label, "gpu0"));
    assert(hwm_is_visible(&data, hwmon_temp, 99, 0) == 0);
    assert(hwm_read(&dev, hwmon_temp, hwmon_temp_label, 0, &value) == -EOPNOTSUPP);
    return 0;
}
