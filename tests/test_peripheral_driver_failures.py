"""Compile actual driver functions with fault-injected kernel API boundaries."""

from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CC = shutil.which("cc")
ITE_MODULES = ("ite_8291", "ite_8291_lb", "ite_8297", "ite_829x")


def source(module):
    return (ROOT / "src" / module / f"{module}.c").read_text()


def function(text, name):
    match = re.search(r"^static [^\n]*\b" + name + r"\([^;{]*\)\s*\{", text, re.M)
    if not match:
        raise AssertionError(f"Function definition missing: {name}")
    end, depth = match.end(), 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[match.start():end]


PRELUDE = r"""
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
#define GFP_KERNEL 0
#define pr_debug(...) ((void)0)
#define pr_err(...) ((void)0)
"""


@unittest.skipUnless(CC, "a C compiler is required")
class PeripheralDriverFailures(unittest.TestCase):
    def run_c(self, code):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "driver.c"
            binary = Path(tmp) / "driver"
            src.write_text(PRELUDE + code)
            result = subprocess.run(
                [CC, "-std=c11", "-Wall", "-Wextra", "-Werror",
                 "-Wno-unused-parameter", "-fsanitize=undefined",
                 "-fno-sanitize-recover=all", str(src), "-o", str(binary)],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            subprocess.run([str(binary)], check=True, capture_output=True, text=True)

    def test_hid_allocation_short_transfer_and_transport_errors(self):
        for module in ITE_MODULES:
            with self.subTest(module=module):
                text = source(module)
                size = re.search(r"#define HID_DATA_SIZE\s+(\d+)", text)[1]
                name, call = {
                    "ite_8291": ("ite8291_write_control", "ite8291_write_control(dev, payload)"),
                    "ite_8291_lb": ("ite8291_write_control", "ite8291_write_control(dev, payload)"),
                    "ite_8297": ("ite8297_write_color", "ite8297_write_color(dev, 1, 2, 3)"),
                    "ite_829x": ("keyb_send_data", "keyb_send_data(dev, 1, 2, 3, 4, 5)"),
                }[module]
                payload = "u8 payload[HID_DATA_SIZE] = {1};" if "control" in name else ""
                self.run_c(f"#define HID_DATA_SIZE {size}\n" + r"""
struct hid_device { int unused; };
#define HID_FEATURE_REPORT 0
#define HID_REQ_SET_REPORT 0
static int locked, allocation_fails, frees, calls, raw_result;
#define mutex_lock(p) ((void)(++locked))
#define mutex_unlock(p) ((void)(--locked))
static void *kzalloc(size_t size, int flags) {
    return allocation_fails ? NULL : calloc(1, size);
}
static void kfree(void *p) { ++frees; free(p); }
static int hid_hw_raw_request(struct hid_device *dev, u8 report, u8 *buf,
                             size_t len, int type, int req) {
    assert(buf && len == HID_DATA_SIZE); ++calls; return raw_result;
}
""" + function(text, name) + "\nstatic int invoke(struct hid_device *dev) {\n" +
                           payload + "\nreturn " + call + ";\n}\n" + r"""
int main(void) {
    struct hid_device dev = {0};
    assert(invoke(NULL) == -ENODEV && calls == 0);
    allocation_fails = 1;
    assert(invoke(&dev) == -ENOMEM && calls == 0 && frees == 0 && locked == 0);
    allocation_fails = 0;
    raw_result = -EPIPE;
    assert(invoke(&dev) == -EPIPE && calls == 1 && frees == 1 && locked == 0);
    raw_result = HID_DATA_SIZE - 1;
    assert(invoke(&dev) == -EIO && calls == 2 && frees == 2 && locked == 0);
    raw_result = HID_DATA_SIZE;
    assert(invoke(&dev) == 0 && calls == 3 && frees == 3 && locked == 0);
    return 0;
}
""")

    def test_hid_start_unwinds_only_acquired_resources(self):
        for module in ITE_MODULES:
            with self.subTest(module=module):
                text = source(module)
                global_device = "static struct hid_device *kbdev;\n" if module == "ite_829x" else ""
                self.run_c(r"""
struct hid_device { int unused; };
#define HID_CONNECT_DEFAULT 0
#define PM_HINT_FULLON 1
#define PM_HINT_NORMAL 0
static int start_result, power_result, open_result;
static int starts, powers, normals, opens, closes, stops;
static int hid_hw_start(struct hid_device *dev, int mask) { ++starts; return start_result; }
static int hid_hw_power(struct hid_device *dev, int hint) {
    if (hint == PM_HINT_FULLON) { ++powers; return power_result; }
    ++normals; return 0;
}
static int hid_hw_open(struct hid_device *dev) { ++opens; return open_result; }
static void hid_hw_close(struct hid_device *dev) { ++closes; }
static void hid_hw_stop(struct hid_device *dev) { ++stops; }
static void reset(void) {
    starts = powers = normals = opens = closes = stops = 0;
    start_result = power_result = open_result = 0;
}
""" + global_device + function(text, "start_hw") + "\n" + function(text, "stop_hw") + r"""
int main(void) {
    struct hid_device dev = {0};
    reset(); start_result = -EIO;
    assert(start_hw(&dev) == -EIO && starts == 1 && powers == 0 && closes == 0 && stops == 0);
    reset(); power_result = -EBUSY;
    assert(start_hw(&dev) == -EBUSY && powers == 1 && opens == 0 && closes == 0 && stops == 1);
    reset(); open_result = -EPIPE;
    assert(start_hw(&dev) == -EPIPE && opens == 1 && normals == 1 && closes == 0 && stops == 1);
    reset(); assert(start_hw(&dev) == 0 && opens == 1 && stops == 0 && closes == 0);
    stop_hw(&dev); assert(normals == 1 && closes == 1 && stops == 1);
    return 0;
}
""")

    def test_perkey_registration_failure_releases_exactly_registered_leds(self):
        self.run_c(r"""
#define ITE8291_NR_ROWS 6
#define ITE8291_LEDS_PER_ROW_MAX 21
#define ITE8291_KBD_BRIGHTNESS_MAX 50
#define ITE8291_KBD_BRIGHTNESS_DEFAULT 0
#define ITE8291_KB_COLOR_DEFAULT_RED 255
#define ITE8291_KB_COLOR_DEFAULT_GREEN 255
#define ITE8291_KB_COLOR_DEFAULT_BLUE 255
#define LED_FUNCTION_KBD_BACKLIGHT "kbd_backlight"
#define LED_COLOR_ID_RED 0
#define LED_COLOR_ID_GREEN 1
#define LED_COLOR_ID_BLUE 2
enum led_brightness { LED_OFF = 0 };
struct led_classdev {
    const char *name; int max_brightness, brightness;
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
};
struct mc_subled { int color_index, intensity, channel; };
struct led_classdev_mc { struct led_classdev led_cdev; int num_colors; struct mc_subled *subled_info; };
struct ite8291_driver_data_perkey_t {
    struct led_classdev_mc mcled_cdevs[6][21];
    struct mc_subled mcled_cdevs_subleds[6][21][3];
};
struct ite8291_driver_data_t { struct ite8291_driver_data_perkey_t *device_data; };
struct hid_device { int dev; };
static struct ite8291_driver_data_perkey_t data;
static struct ite8291_driver_data_t driver = {&data};
static int fail_at, calls, releases, active[126];
static void *hid_get_drvdata(struct hid_device *dev) { return &driver; }
static int leds_set_brightness_mc(struct led_classdev *led, enum led_brightness value) { return 0; }
static int index_of(struct led_classdev_mc *led) {
    return ((uintptr_t)led - (uintptr_t)&data.mcled_cdevs[0][0]) / sizeof(*led);
}
static int devm_led_classdev_multicolor_register(void *dev, struct led_classdev_mc *led) {
    int index = index_of(led);
    assert(index == calls++);
    if (index == fail_at) return -EIO;
    active[index] = 1; return 0;
}
static void devm_led_classdev_multicolor_unregister(void *dev, struct led_classdev_mc *led) {
    int index = index_of(led); assert(index >= 0 && index < 126 && active[index]);
    active[index] = 0; ++releases;
}
""" + function(source("ite_8291"), "register_leds") + r"""
int main(void) {
    struct hid_device dev = {0};
    for (fail_at = 0; fail_at < 126; ++fail_at) {
        calls = releases = 0; memset(active, 0, sizeof(active));
        assert(register_leds(&dev) == -EIO && calls == fail_at + 1 && releases == fail_at);
        for (int i = 0; i < 126; ++i) assert(active[i] == 0);
    }
    calls = releases = 0; fail_at = -1;
    assert(register_leds(&dev) == 0 && calls == 126 && releases == 0);
    return 0;
}
""")

    def test_firmware_orientation_rejects_invalid_axes_without_partial_update(self):
        self.run_c(r"""
#define ACPI_ALLOCATE_BUFFER 0
#define ACPI_TYPE_BUFFER 1
#define ACPI_FAILURE(status) ((status) != 0)
typedef int acpi_status;
struct device { int unused; };
struct acpi_device { void *handle; };
struct acpi_buffer { unsigned int length; void *pointer; };
union acpi_object {
    int type;
    struct { int type; unsigned int length; u8 *pointer; } buffer;
};
struct iio_mount_matrix { const char *rotation[9]; };
static const struct iio_mount_matrix iio_mount_zeromatrix = {
    .rotation = {"0", "0", "0", "0", "0", "0", "0", "0", "0"}
};
static struct acpi_device device;
static u8 axes[3];
static int releases, firmware_length = 3;
#define ACPI_COMPANION(dev) (&device)
static bool acpi_has_method(void *handle, char *name) { return true; }
static int acpi_evaluate_object(void *handle, char *method, void *args, struct acpi_buffer *buffer) {
    union acpi_object *obj = calloc(1, sizeof(*obj));
    obj->type = ACPI_TYPE_BUFFER; obj->buffer.length = firmware_length; obj->buffer.pointer = axes;
    buffer->pointer = obj; return 0;
}
static void kfree(void *pointer) { ++releases; free(pointer); }
""" + function(source("stk8321"), "stk8321_apply_acpi_orientation") + r"""
int main(void) {
    struct device dev = {0};
    struct iio_mount_matrix original, matrix;
    for (int i = 0; i < 9; ++i) original.rotation[i] = "old";
    for (int invalid = 0; invalid < 3; ++invalid) {
        axes[0] = 0; axes[1] = 1; axes[2] = 2; axes[invalid] = 3;
        matrix = original; releases = 0;
        assert(stk8321_apply_acpi_orientation(&dev, "GETO", &matrix) == -EINVAL);
        assert(memcmp(&matrix, &original, sizeof(matrix)) == 0 && releases == 1);
    }
    firmware_length = 2; matrix = original; releases = 0;
    assert(stk8321_apply_acpi_orientation(&dev, "GETO", &matrix) == -EIO && releases == 1);
    assert(memcmp(&matrix, &original, sizeof(matrix)) == 0);
    firmware_length = 3; axes[0] = 1; axes[1] = 0x10; axes[2] = 2;
    assert(stk8321_apply_acpi_orientation(&dev, "GETO", &matrix) == 0);
    for (int i = 0; i < 9; ++i)
        assert(strcmp(matrix.rotation[i], i == 1 ? "-1" : (i == 3 || i == 8) ? "1" : "0") == 0);
    return 0;
}
""")

    def test_rgb_probe_releases_registered_leds_before_stopping_hardware(self):
        self.run_c(r"""
#define LED_NAME_RGB_RED "red"
#define LED_NAME_RGB_GREEN "green"
#define LED_NAME_RGB_BLUE "blue"
#define LED_MAX_BRIGHTNESS 255
#define ITE_8297_DEFAULT_BRIGHTNESS 0
enum led_brightness { LED_OFF = 0 };
struct led_classdev {
    const char *name; int max_brightness;
    int (*brightness_set_blocking)(struct led_classdev *, enum led_brightness);
    enum led_brightness (*brightness_get)(struct led_classdev *);
};
struct hid_device { int dev; };
struct hid_device_id { int unused; };
struct color_t { u8 red, green, blue; };
struct ite8297_driver_data_t {
    struct led_classdev cdev_red, cdev_green, cdev_blue;
    struct hid_device *hid_dev; struct color_t current_color; int lock;
};
static int failure, starts, stops, registers, releases, active[3];
static struct ite8297_driver_data_t *allocated;
#define mutex_init(p) ((void)0)
static void *devm_kzalloc(void *dev, size_t size, int flags) {
    allocated = failure == 1 ? NULL : calloc(1, size); return allocated;
}
static int hid_parse(struct hid_device *dev) { return failure == 2 ? -EIO : 0; }
static int start_hw(struct hid_device *dev) { ++starts; return failure == 3 ? -EIO : 0; }
static void stop_hw(struct hid_device *dev) {
    assert(active[0] == 0 && active[1] == 0 && active[2] == 0); ++stops;
}
static void hid_set_drvdata(struct hid_device *dev, void *data) { assert(data == allocated); }
static int lightbar_set_blocking(struct led_classdev *led, enum led_brightness b) { return 0; }
static enum led_brightness lightbar_get(struct led_classdev *led) { return 0; }
static int led_classdev_register(void *dev, struct led_classdev *led) {
    int index = registers++;
    if (failure == index + 4) return -EIO;
    active[index] = 1; return 0;
}
static void led_classdev_unregister(struct led_classdev *led) {
    int index = led == &allocated->cdev_red ? 0 : led == &allocated->cdev_green ? 1 : 2;
    assert(active[index] == 1); active[index] = 0; ++releases;
}
static int ite8297_write_state(struct ite8297_driver_data_t *data) { return failure == 7 ? -EIO : 0; }
""" + function(source("ite_8297"), "driver_probe_callb") + r"""
int main(void) {
    struct hid_device dev = {0};
    for (failure = 1; failure <= 7; ++failure) {
        starts = stops = registers = releases = 0; memset(active, 0, sizeof(active));
        assert(driver_probe_callb(&dev, NULL) == (failure == 1 ? -ENOMEM : -EIO));
        assert(stops == (failure >= 4));
        assert(releases == (failure >= 4 ? failure - 4 : 0));
        assert(active[0] == 0 && active[1] == 0 && active[2] == 0);
        free(allocated);
    }
    failure = 0; starts = stops = registers = releases = 0;
    assert(driver_probe_callb(&dev, NULL) == 0 && registers == 3 && stops == 0);
    free(allocated); return 0;
}
""")

    def test_keyboard_remove_drains_pending_work_before_device_teardown(self):
        self.run_c(r"""
#define KEYBOARD_ROWS 6
#define KEYBOARD_COLUMNS 20
struct hid_device { int dev; };
struct led_classdev_mc { int unused; };
static struct led_classdev_mc clevo_mcled_cdevs[6][20];
static int keyboard_notifier_block, ite_829x_key_work, stage, releases;
static void unregister_keyboard_notifier(void *notifier) { assert(stage == 0); stage = 1; }
static void cancel_work_sync(void *work) { assert(stage == 1); stage = 2; }
static void devm_led_classdev_multicolor_unregister(void *dev, struct led_classdev_mc *led) {
    assert(stage == 2); ++releases;
}
static void stop_hw(struct hid_device *dev) { assert(stage == 2 && releases == 120); stage = 3; }
""" + function(source("ite_829x"), "remove_callb") + r"""
int main(void) {
    struct hid_device dev = {0}; remove_callb(&dev); assert(stage == 3); return 0;
}
""")

    def test_acpi_lookup_reference_is_released_on_success_and_registration_failure(self):
        self.run_c(r"""
#define __init
#define DRIVER_NAME "gxtp7380"
#define PLATFORM_DEVID_NONE -1
#define IS_ERR(p) ((intptr_t)(p) < 0)
#define PTR_ERR(p) ((int)(intptr_t)(p))
struct acpi_device { int unused; };
struct platform_device { int unused; };
struct platform_device_info { const char *name; int id; void *fwnode; };
static struct platform_device *pdev, platform;
static struct acpi_device acpi;
static int gxtp7380_driver, failure, references, unregisters;
static struct acpi_device *acpi_dev_get_first_match_dev(const char *hid, void *uid, int hrv) {
    if (failure == 1) return NULL;
    ++references; return &acpi;
}
static void *acpi_fwnode_handle(struct acpi_device *dev) { return dev; }
static void acpi_dev_put(struct acpi_device *dev) { --references; }
static struct platform_device *platform_device_register_full(struct platform_device_info *info) {
    assert(references == 1); return failure == 2 ? (void *)(intptr_t)-ENOMEM : &platform;
}
static int platform_driver_register(void *driver) { return failure == 3 ? -EIO : 0; }
static void platform_device_unregister(struct platform_device *dev) { ++unregisters; }
""" + function(source("gxtp7380"), "gxtp7380_driver_init") + r"""
int main(void) {
    for (failure = 0; failure <= 3; ++failure) {
        references = unregisters = 0;
        assert(gxtp7380_driver_init() == (failure == 1 ? -ENODEV : failure == 2 ? -ENOMEM : failure == 3 ? -EIO : 0));
        assert(references == 0 && unregisters == (failure == 3));
    }
    return 0;
}
""")

    def test_sensor_frequency_preserves_errors_and_actual_configured_value(self):
        self.run_c(r"""
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define IIO_CHAN_INFO_SAMP_FREQ 1
#define STK8321_POWMODE_SUSPEND 128
#define STK8321_POWMODE_NORMAL 0
struct i2c_client { int unused; };
struct stk8321_data { struct i2c_client *client; int lock, samp_freq; };
struct iio_dev { struct stk8321_data *data; };
struct iio_chan_spec { int unused; };
static const struct { int val, val2; u8 reg_bits; } stk8321_samp_freq_table[] = {
    {7, 810000, 8}, {15, 630000, 9}, {31, 250000, 10}, {62, 500000, 11},
    {125, 0, 12}, {250, 0, 13}, {500, 0, 14}, {1000, 0, 15}
};
static int locked, calls, fail_at;
#define mutex_lock(p) ((void)(++locked))
#define mutex_unlock(p) ((void)(--locked))
static void *iio_priv(struct iio_dev *dev) { return dev->data; }
static int stk8321_set_power_mode(struct i2c_client *client, int mode) {
    ++calls; assert(mode == (calls == 1 ? 128 : 0)); return calls == fail_at ? -EIO : 0;
}
static int stk8321_set_bandwidth(struct i2c_client *client, int value) {
    ++calls; assert(value == 12 && calls == 2); return calls == fail_at ? -EIO : 0;
}
""" + function(source("stk8321"), "stk8321_write_raw") + r"""
int main(void) {
    struct i2c_client client = {0};
    struct stk8321_data data = {&client, 0, 0};
    struct iio_dev dev = {&data};
    assert(stk8321_write_raw(&dev, NULL, 99, 0, 1) == -EINVAL && calls == 0);
    for (fail_at = 1; fail_at <= 3; ++fail_at) {
        calls = 0; data.samp_freq = 0;
        assert(stk8321_write_raw(&dev, NULL, 125, 0, 1) == -EIO && locked == 0);
        assert(calls == (fail_at == 1 ? 1 : 3));
        assert(data.samp_freq == (fail_at == 3 ? 4 : 0));
    }
    calls = 0; fail_at = 0;
    assert(stk8321_write_raw(&dev, NULL, 125, 0, 1) == 0 && data.samp_freq == 4 && locked == 0);
    return 0;
}
""")


if __name__ == "__main__":
    unittest.main()
