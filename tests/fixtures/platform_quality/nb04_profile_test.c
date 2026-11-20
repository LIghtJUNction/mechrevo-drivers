
static const struct attribute_group platform_profile_attr_group;
static int firmware_error, firmware_calls, firmware_status;
static u8 captured_profile;
static int nb04_wmi_bs_method(u32 method, u8 *in, u8 *out)
{
    assert(method == 7);
    ++firmware_calls;
    captured_profile = in[0];
    for (unsigned i = 1; i < BS_INPUT_BUFFER_LENGTH; ++i)
        assert(in[i] == 0);
    if (firmware_error)
        return firmware_error;
    memset(out, 0, BS_OUTPUT_BUFFER_LENGTH);
    out[0] = firmware_status;
    return 0;
}

@FUNCTIONS@

int main(void)
{
    char buffer[4096];
    struct driver_data_t data = { .current_profile_value = WMI_SYSTEM_MODE_BEAST };
    mutex_init(&data.lock);
    struct device dev = { .drvdata = &data };

    memset(buffer, 'x', sizeof(buffer));
    assert(platform_profile_choices_show(&dev, NULL, buffer) ==
           strlen("low-power balanced performance\n"));
    assert(!strcmp(buffer, "low-power balanced performance\n"));
    firmware_error = -ENODEV;
    assert(platform_profile_store(&dev, NULL, "balanced\n", 9) == -ENODEV);
    assert(data.current_profile_value == WMI_SYSTEM_MODE_BEAST);
    firmware_error = 0;
    firmware_status = WMI_RETURN_STATUS_UNSUPPORTED;
    assert(platform_profile_store(&dev, NULL, "balanced\n", 9) == -EIO);
    assert(data.current_profile_value == WMI_SYSTEM_MODE_BEAST);
    firmware_status = 0;
    assert(platform_profile_store(&dev, NULL, "balanced\n", 9) == 9);
    assert(data.current_profile_value == WMI_SYSTEM_MODE_HUMAN);
    assert(captured_profile == WMI_SYSTEM_MODE_HUMAN);
    assert(platform_profile_show(&dev, NULL, buffer) == 9);
    assert(!strcmp(buffer, "balanced\n"));
    int before = firmware_calls;
    assert(platform_profile_store(&dev, NULL, "balanced-more", 13) == -EINVAL);
    assert(platform_profile_store(&dev, NULL, "", 0) == -EINVAL);
    assert(set_system_mode(WMI_SYSTEM_MODE_END) == -EINVAL);
    assert(firmware_calls == before);

    struct platform_device pdev = {0};
    allocation_fails = true;
    assert(tuxedo_nb04_power_profiles_probe(&pdev) == -ENOMEM);
    assert(group_calls == 0 && firmware_calls == before);
    allocation_fails = false;
    firmware_error = -EIO;
    assert(tuxedo_nb04_power_profiles_probe(&pdev) == -EIO);
    assert(group_calls == 0);
    pthread_mutex_destroy(&((struct driver_data_t *)pdev.dev.drvdata)->lock.native);
    free(pdev.dev.drvdata);
    firmware_error = 0;
    group_result = -ENOMEM;
    assert(tuxedo_nb04_power_profiles_probe(&pdev) == -ENOMEM);
    assert(group_calls == 1);
    pthread_mutex_destroy(&((struct driver_data_t *)pdev.dev.drvdata)->lock.native);
    free(pdev.dev.drvdata);
    pthread_mutex_destroy(&data.lock.native);
    return 0;
}
