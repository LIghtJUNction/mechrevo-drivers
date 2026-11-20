/* Kernel/firmware substitutes; the implementation functions are inserted below. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
typedef uint64_t u64;
typedef size_t acpi_size;
typedef int acpi_status;
typedef void *acpi_handle;
typedef struct { int unused; } guid_t;
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_BUFFER 3
#define ACPI_TYPE_PACKAGE 4
#define ACPI_ALLOCATE_BUFFER ((size_t)-1)
#define ACPI_FAILURE(s) ((s) != 0)
#define ACPI_ALL_NOTIFY 3
#define GFP_KERNEL 0
#define unlikely(v) (v)
#define IS_ERR_OR_NULL(p) (!(p) || (uintptr_t)(p) >= (uintptr_t)-4095)
#define pr_err(...) do {} while (0)
#define pr_info(...) do {} while (0)
#define pr_debug(...) do {} while (0)
/* @CONSTANTS@ */
union acpi_object {
    u32 type;
    struct { u32 type; u64 value; } integer;
    struct { u32 type, length; u8 *pointer; } buffer;
    struct { u32 type, count; union acpi_object *elements; } package;
};
struct acpi_buffer { size_t length; void *pointer; };
struct acpi_device { acpi_handle handle; };
struct wmi_device { int unused; };
struct device { struct acpi_device *companion; };
struct platform_device { struct device dev; };
struct clevo_interface_t {
    char *string_id;
    void (*event_callb)(u32);
    int (*method_call)(u8, u32, union acpi_object **);
    int (*method_call_pkgbuf)(u8, u8 *, u32, union acpi_object **);
};
static struct clevo_interface_t clevo_wmi_interface, clevo_acpi_interface;
struct clevo_platform_driver_data_t {
    struct acpi_device *adev;
    struct clevo_interface_t *clevo_interface;
};
static struct clevo_platform_driver_data_t *active_driver_data, driver_data;
static union acpi_object *response;
static void *live[32];
static int allocations, frees, transport_status, method_calls, parse_error;
static int events;
static u32 last_event;
static union acpi_object *object(u32 type, u64 value)
{
    union acpi_object *result = calloc(1, sizeof(*result));
    assert(result && allocations < 32);
    live[allocations++] = result;
    result->integer.type = type;
    result->integer.value = value;
    return result;
}
static void free_object(void *pointer)
{
    if (!pointer) return;
    int i;
    for (i = 0; i < allocations && live[i] != pointer; ++i) {}
    assert(i < allocations);
    live[i] = NULL;
    ++frees;
    free(pointer);
}
#define ACPI_FREE(p) free_object(p)
static acpi_status wmi_evaluate_method(const char *guid, int instance, u32 method,
                                      struct acpi_buffer *in, struct acpi_buffer *out)
{
    assert(in->length == sizeof(u32));
    assert(out->length == ACPI_ALLOCATE_BUFFER);
    ++method_calls;
    out->pointer = response;
    response = NULL;
    return transport_status;
}
static int guid_parse(const char *text, guid_t *guid) { return parse_error; }
static acpi_handle acpi_device_handle(struct acpi_device *device) { return device->handle; }
static union acpi_object *acpi_evaluate_dsm(acpi_handle handle, guid_t *guid,
                                          u64 revision, u64 cmd, union acpi_object *args)
{
    union acpi_object *result = response;
    assert(handle && args->type == ACPI_TYPE_PACKAGE && args->package.count == 1);
    ++method_calls;
    response = NULL;
    return result;
}
static void event_callback(u32 value) { ++events; last_event = value; }
/* @FUNCTION clevo_wmi.c clevo_wmi_evaluate@ */
/* @FUNCTION clevo_wmi.c clevo_wmi_interface_method_call_pkgbuf@ */
/* @FUNCTION clevo_wmi.c clevo_wmi_notify@ */
/* @FUNCTION clevo_acpi.c clevo_acpi_evaluate@ */
/* @FUNCTION clevo_acpi.c clevo_acpi_evaluate_pkgbuf@ */
/* @FUNCTION clevo_acpi.c clevo_acpi_notify@ */
static bool allocation_error, notify_installed;
static int notify_error, binding_error, installs, uninstalls, binds;
#define ACPI_COMPANION(d) ((d)->companion)
static void *devm_kzalloc(struct device *dev, size_t size, int flags)
{
    assert(size == sizeof(driver_data));
    memset(&driver_data, 0, sizeof(driver_data));
    return allocation_error ? NULL : &driver_data;
}
static int acpi_dev_install_notify_handler(struct acpi_device *acpi, int type,
        void (*handler)(acpi_handle, u32, void *), void *data)
{
    assert(type == ACPI_ALL_NOTIFY && data == acpi && handler == clevo_acpi_notify);
    ++installs;
    notify_installed = !notify_error;
    return notify_error;
}
static void acpi_dev_remove_notify_handler(struct acpi_device *acpi, int type,
        void (*handler)(acpi_handle, u32, void *))
{
    assert(notify_installed);
    ++uninstalls;
    notify_installed = false;
}
static int clevo_keyboard_add_interface(struct clevo_interface_t *interface)
{
    assert(interface == &clevo_acpi_interface && active_driver_data == &driver_data);
    assert(notify_installed);
    ++binds;
    return binding_error;
}
/* @FUNCTION clevo_acpi.c clevo_platform_probe@ */

int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *name = argv[1];
    struct acpi_device acpi = { .handle = (void *)1 };
    struct platform_device pdev = { .dev = { .companion = &acpi } };
    union acpi_object *out = (void *)1;
    u8 arg = 0x19;
    clevo_wmi_interface.event_callb = clevo_acpi_interface.event_callb = event_callback;
    if (!strcmp(name, "wmi_error_cleanup")) {
        response = object(ACPI_TYPE_INTEGER, 7); transport_status = 1;
        assert(clevo_wmi_evaluate(1, 0, &out) == -EIO);
        assert(out == NULL && allocations == frees);
    } else if (!strcmp(name, "wmi_absent_result")) {
        assert(clevo_wmi_evaluate(1, 0, &out) == -ENODATA && out == NULL);
    } else if (!strcmp(name, "wmi_discard_result")) {
        response = object(ACPI_TYPE_INTEGER, 7);
        assert(!clevo_wmi_evaluate(1, 0, NULL) && allocations == frees);
    } else if (!strcmp(name, "wmi_transfer_result")) {
        response = object(ACPI_TYPE_INTEGER, 0x12345678);
        assert(!clevo_wmi_evaluate(1, 0, &out) && out->integer.value == 0x12345678);
        assert(frees == 0); ACPI_FREE(out);
    } else if (!strcmp(name, "wmi_invalid_events")) {
        transport_status = 1;
        clevo_wmi_notify(NULL, NULL);
        transport_status = 0;
        clevo_wmi_notify(NULL, NULL);
        response = object(ACPI_TYPE_BUFFER, 0);
        clevo_wmi_notify(NULL, NULL);
        assert(events == 0 && allocations == frees);
    } else if (!strcmp(name, "wmi_valid_event")) {
        response = object(ACPI_TYPE_INTEGER, 0x5d);
        clevo_wmi_notify(NULL, NULL);
        assert(events == 1 && last_event == 0x5d && allocations == frees);
    } else if (!strcmp(name, "wmi_unsupported_package")) {
        assert(clevo_wmi_interface_method_call_pkgbuf(4, &arg, 1, &out) == -EOPNOTSUPP);
        assert(out == NULL && !method_calls);
    } else if (!strcmp(name, "acpi_discard_result")) {
        response = object(ACPI_TYPE_INTEGER, 0);
        assert(!clevo_acpi_evaluate(&acpi, 1, 0, NULL));
        response = object(ACPI_TYPE_INTEGER, 0);
        assert(!clevo_acpi_evaluate_pkgbuf(&acpi, 4, &arg, 1, NULL));
        assert(allocations == frees);
    } else if (!strcmp(name, "acpi_transfer_result")) {
        response = object(ACPI_TYPE_INTEGER, 10);
        assert(!clevo_acpi_evaluate(&acpi, 1, 0, &out) && out->integer.value == 10);
        ACPI_FREE(out);
        response = object(ACPI_TYPE_INTEGER, 11);
        assert(!clevo_acpi_evaluate_pkgbuf(&acpi, 4, &arg, 1, &out) && out->integer.value == 11);
        ACPI_FREE(out);
    } else if (!strcmp(name, "acpi_failures")) {
        assert(clevo_acpi_evaluate(&acpi, 1, 0, &out) == -EIO && out == NULL);
        assert(clevo_acpi_evaluate_pkgbuf(&acpi, 4, &arg, 1, &out) == -EIO && out == NULL);
        acpi.handle = NULL;
        assert(clevo_acpi_evaluate(&acpi, 1, 0, &out) == -ENODEV);
        assert(clevo_acpi_evaluate_pkgbuf(&acpi, 4, &arg, 1, &out) == -ENODEV);
        parse_error = -EINVAL;
        assert(clevo_acpi_evaluate(&acpi, 1, 0, &out) == -ENOENT);
    } else if (!strcmp(name, "acpi_event_identity")) {
        for (int i = 0; i < 3; ++i) {
            if (i) response = object(i == 1 ? ACPI_TYPE_INTEGER : ACPI_TYPE_BUFFER, 0x99);
            clevo_acpi_notify(acpi.handle, 0x5d, &acpi);
            assert(last_event == 0x5d);
        }
        assert(events == 3 && allocations == frees);
    } else if (!strncmp(name, "probe_", 6)) {
        int expected = 0;
        if (!strcmp(name, "probe_alloc_failure")) { allocation_error = true; expected = -ENOMEM; }
        else if (!strcmp(name, "probe_notify_failure")) { notify_error = -EIO; expected = -EIO; }
        else if (!strcmp(name, "probe_binding_failure")) { binding_error = -EBUSY; expected = -EBUSY; }
        else assert(!strcmp(name, "probe_success"));
        assert(clevo_platform_probe(&pdev) == expected);
        if (expected) {
            assert(!active_driver_data && !notify_installed);
            assert(uninstalls == (binding_error ? 1 : 0));
            if (allocation_error) assert(installs == 0 && binds == 0);
        } else {
            assert(active_driver_data == &driver_data && notify_installed && binds == 1);
        }
    } else return 2;
    assert(allocations == frees);
    return 0;
}
