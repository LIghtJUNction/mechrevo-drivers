
static struct nb05_device_data_t device_data = { .number_fans = 2 };
static struct nb05_ec_data_t ec_data = { .dev_data = &device_data };
static const struct attribute_group fan_control_attr_group;
static u8 registers[65536];
static int writes;
static void nb05_read_ec_ram(u16 address, u8 *value) { *value = registers[address]; }
static void nb05_write_ec_ram(u16 address, u8 value)
{
    ++writes;
    registers[address] = value;
}
static void nb05_get_ec_data(struct nb05_ec_data_t **data) { *data = &ec_data; }

@FUNCTIONS@

static void check_version(u8 major, u8 minor, bool one_register, bool expects_rpm)
{
    struct platform_device pdev = {0};
    ec_data.ver_major = major;
    ec_data.ver_minor = minor;
    device_data.fanctl_onereg = one_register;
    assert(tuxedo_nb05_fan_control_probe(&pdev) == group_result);
    struct driver_data_t *data = pdev.dev.drvdata;
    assert(data->write_rpm == expects_rpm);
    free(data);
}
int main(void)
{
    check_version(8, 99, false, true);
    check_version(9, 9, false, true);
    check_version(9, 10, false, false);
    check_version(10, 0, false, false);
    check_version(8, 1, true, false);
    group_result = -ENOMEM;
    check_version(9, 10, false, false);
    assert(group_calls == 6);

    char buffer[4096];
    struct driver_data_t data = { .ec_data = &ec_data };
    struct device dev = { .drvdata = &data };
    device_data.fanctl_onereg = false;
    registers[0x2c1] = 0xb8;
    assert(fan1_pwm_show(&dev, NULL, buffer) == 4 && !strcmp(buffer, "255\n"));
    registers[0x2c1] = 0xb9;
    assert(fan1_pwm_show(&dev, NULL, buffer) == -ERANGE);
    registers[0x241] = 0xff;
    assert(fan2_pwm_show(&dev, NULL, buffer) == -ERANGE);
    registers[0x241] = 0;
    assert(fan2_pwm_show(&dev, NULL, buffer) == 2 && !strcmp(buffer, "0\n"));
    device_data.fanctl_onereg = true;
    registers[0x1809] = 0xb8;
    assert(fan1_pwm_show(&dev, NULL, buffer) == 4 && !strcmp(buffer, "255\n"));
    assert(fan1_pwm_enable_store(&dev, NULL, "1", 1) == 1);
    assert(registers[0x2f1] == 0xaa);
    assert(fan1_pwm_enable_store(&dev, NULL, "2", 1) == 1);
    assert(registers[0x2f1] == 0);
    assert(fan2_pwm_enable_store(&dev, NULL, "1", 1) == 1 && registers[0x240] == 1);
    int before = writes;
    assert(fan1_pwm_enable_store(&dev, NULL, "0", 1) == -EINVAL);
    assert(fan1_pwm_enable_store(&dev, NULL, "3", 1) == -EINVAL);
    assert(fan2_pwm_enable_store(&dev, NULL, "0", 1) == -EINVAL);
    assert(fan2_pwm_enable_store(&dev, NULL, "255", 3) == -EINVAL);
    assert(writes == before);
    return 0;
}
