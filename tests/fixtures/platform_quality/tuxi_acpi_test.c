
static unsigned long long firmware_value;
static unsigned long long captured_args[2];
static unsigned captured_count;
static const char *captured_method;
static int firmware_error, firmware_calls;

static acpi_status acpi_evaluate_integer(acpi_handle handle, acpi_string method,
                                         struct acpi_object_list *input,
                                         unsigned long long *result)
{
    assert(handle == (void *)1);
    ++firmware_calls;
    captured_method = method;
    captured_count = input ? input->count : 0;
    for (unsigned i = 0; i < captured_count; ++i) {
        assert(input->pointer[i].type == ACPI_TYPE_INTEGER);
        captured_args[i] = input->pointer[i].integer.value;
    }
    if (firmware_error)
        return firmware_error;
    *result = firmware_value;
    return 0;
}

@FUNCTIONS@

int main(void)
{
    struct tuxi_platform_driver_data_t data = { .tfan_handle = (void *)1 };
    struct platform_device pdev = { .dev.drvdata = &data };
    u8 byte = 77;
    u16 word = 777;
    enum tuxi_fan_mode mode = AUTO;
    enum tuxi_fan_type type = GENERAL;
    unsigned long long result = 123, args[] = { 5, 6, 7 };

    assert(evaluate_intparams(NULL, "GCNT", NULL, 0, &result) == -ENODEV);
    assert(evaluate_intparams((void *)1, "GCNT", args, 3, &result) == -EINVAL);
    assert(evaluate_intparams((void *)1, "GSPD", NULL, 1, &result) == -EINVAL);
    assert(firmware_calls == 0 && result == 123);
    assert(tuxi_get_fan_speed(0, &byte) == -ENODEV);
    tuxi_driver_data = &data;

    firmware_value = 2;
    assert(tuxi_get_nr_fans(&byte) == 0 && byte == 2 && captured_count == 0);
    assert(!strcmp(captured_method, "GCNT"));
    assert(tuxi_get_nr_fans(NULL) == -EINVAL);
    firmware_value = 255;
    assert(tuxi_get_fan_speed(1, &byte) == 0 && byte == 255);
    assert(captured_count == 1 && captured_args[0] == 1);
    firmware_value = 256;
    assert(tuxi_get_fan_speed(1, &byte) == -ERANGE && byte == 255);
    assert(tuxi_get_nr_fans(&byte) == -ERANGE && byte == 255);

    firmware_value = 65535;
    assert(tuxi_get_fan_temp(0, &word) == 0 && word == 65535);
    assert(tuxi_get_fan_rpm(0, &word) == 0 && word == 65535);
    firmware_value = 65536;
    assert(tuxi_get_fan_temp(0, &word) == -ERANGE && word == 65535);
    assert(tuxi_get_fan_rpm(0, &word) == -ERANGE && word == 65535);
    firmware_value = UINT64_MAX;
    assert(tuxi_get_fan_temp(0, &word) == -ERANGE && word == 65535);

    firmware_value = MANUAL;
    assert(tuxi_get_fan_mode(&mode) == 0 && mode == MANUAL);
    firmware_value = 2;
    assert(tuxi_get_fan_mode(&mode) == -EINVAL && mode == MANUAL);
    assert(tuxi_get_fan_type(0, &type) == 0 && type == GPU);
    firmware_value = 3;
    assert(tuxi_get_fan_type(0, &type) == -EINVAL && type == GPU);
    int before = firmware_calls;
    assert(tuxi_set_fan_mode((enum tuxi_fan_mode)2) == -EINVAL);
    assert(tuxi_set_fan_mode((enum tuxi_fan_mode)-1) == -EINVAL);
    assert(firmware_calls == before);

    firmware_value = 0;
    assert(tuxi_set_fan_speed(1, 210) == 0);
    assert(captured_count == 2 && captured_args[0] == 1 && captured_args[1] == 210);
    firmware_value = 1;
    assert(tuxi_set_fan_speed(1, 210) == -EINVAL);
    firmware_error = 1;
    assert(tuxi_get_fan_speed(0, &byte) == -EIO && byte == 255);
    assert(tuxi_get_fan_mode(&mode) == -EIO && mode == MANUAL);
    tuxi_platform_remove(&pdev);
    before = firmware_calls;
    assert(!tuxi_driver_data);
    assert(tuxi_get_nr_fans(&byte) == -ENODEV && firmware_calls == before);
    return 0;
}
