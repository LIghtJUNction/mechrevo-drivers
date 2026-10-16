/* Real driver functions are injected below. These mocks never touch hardware. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef size_t acpi_size;
typedef u32 acpi_status;

#define GFP_KERNEL 0
#define ACPI_ALLOCATE_BUFFER ((acpi_size)-1)
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_BUFFER 3
#define ACPI_FAILURE(status) ((status) != 0)
#define pr_err(...) record_error(__VA_ARGS__)
#define pr_debug(...) ((void)0)
#define IS_ERR_OR_NULL(pointer) ((pointer) == NULL)

struct acpi_buffer { acpi_size length; void *pointer; };
union acpi_object {
	u32 type;
	struct { u32 type, length; u8 *pointer; } buffer;
	struct { u32 type; uint64_t value; } integer;
};

/* @DRIVER_DECLARATIONS@ */

static int uniwill_ec_lock, lock_calls, unlock_calls;
static bool uniwill_ec_direct;
static void mutex_lock(int *lock)
{
	assert(!*lock);
	*lock = 1;
	++lock_calls;
}
static void mutex_unlock(int *lock)
{
	assert(*lock);
	*lock = 0;
	++unlock_calls;
}

static bool allocation_failure;
static int allocation_calls, input_frees, output_frees, wmi_calls;
static void *allocated_input;
static union acpi_object *allocated_output;
static void *kzalloc(size_t length, int flags)
{
	assert(flags == GFP_KERNEL && length == 8);
	++allocation_calls;
	if (allocation_failure)
		return NULL;
	assert(!allocated_input);
	allocated_input = calloc(1, length);
	assert(allocated_input);
	return allocated_input;
}
static void kfree(void *pointer)
{
	if (!pointer)
		return;
	if (pointer == allocated_input) {
		allocated_input = NULL;
		++input_frees;
	} else {
		assert(pointer == allocated_output);
		allocated_output = NULL;
		++output_frees;
	}
	free(pointer);
}

static acpi_status acpi_result;
static bool missing_object, null_payload;
static u32 response_type = ACPI_TYPE_BUFFER, response_length = 40;
static u8 payload[128], request[8];
static int wmi_evaluate_method(const char *guid, u8 instance, u32 method,
			       struct acpi_buffer *input, struct acpi_buffer *output)
{
	assert(uniwill_ec_lock);
	assert(strcmp(guid, UNIWILL_WMI_MGMT_GUID_BC) == 0);
	assert(instance == 0 && method == 4);
	assert(input->length == 8 && input->pointer == allocated_input);
	assert(output->length == ACPI_ALLOCATE_BUFFER && !output->pointer);
	memcpy(request, input->pointer, sizeof(request));
	++wmi_calls;
	if (!missing_object) {
		assert(!allocated_output);
		allocated_output = calloc(1, sizeof(*allocated_output));
		assert(allocated_output);
		allocated_output->buffer.type = response_type;
		allocated_output->buffer.length = response_length;
		allocated_output->buffer.pointer = null_payload ? NULL : payload;
		output->pointer = allocated_output;
	}
	return acpi_result;
}

struct ec_event { char op; u8 reg, value; int status; };
static struct ec_event ec_events[128];
static u8 ec_memory[256];
static int ec_count, ec_poll_count, ec_ready_at = 1, ec_fail_at = -1;
static int ec_fault_errno = -EIO, ec_cleanup_error, release_errors, release_errno, timeout_errors;
static bool ec_triggered, ec_land_on_failure;
static void record_error(const char *format, ...)
{
    if (strstr(format, "Could not release direct EC")) {
        va_list args; va_start(args, format);
        release_errno = va_arg(args, int); va_end(args); ++release_errors;
    } else if (strstr(format, "timeout")) ++timeout_errors;
}
static void msleep(unsigned int delay) { assert(delay == UW_EC_BUSY_WAIT_DELAY); }
static int ec_read(u8 reg, u8 *value)
{
    assert(uniwill_ec_lock && ec_count < 128);
    int status = ec_count == ec_fail_at ? ec_fault_errno : 0;
    u8 result = ec_memory[reg];
    if (reg == UNIWILL_EC_REG_FLAGS && ec_triggered) {
        ++ec_poll_count;
        if (ec_ready_at > 0 && ec_poll_count >= ec_ready_at) result |= 1 << UNIWILL_EC_BIT_DRDY;
    }
    ec_events[ec_count++] = (struct ec_event){'r', reg, result, status};
    if (!status) *value = result; /* Failed reads leave caller untouched. */
    return status;
}
static int ec_write(u8 reg, u8 value)
{
    assert(uniwill_ec_lock && ec_count < 128);
    int status = ec_count == ec_fail_at ? ec_fault_errno : 0;
    if (reg == UNIWILL_EC_REG_FLAGS && !value && ec_cleanup_error) status = ec_cleanup_error;
    ec_events[ec_count++] = (struct ec_event){'w', reg, value, status};
    if (!status || ec_land_on_failure) {
        ec_memory[reg] = value;
        if (reg == UNIWILL_EC_REG_FLAGS)
            ec_triggered = !!(value & ((1 << UNIWILL_EC_BIT_RFLG) | (1 << UNIWILL_EC_BIT_WFLG)));
    }
    return status;
}
static void reset_ec(void)
{
    assert(!uniwill_ec_lock);
    memset(ec_memory, 0, sizeof(ec_memory)); memset(ec_events, 0, sizeof(ec_events));
    ec_memory[UNIWILL_EC_REG_FLAGS] = 0x20;
    ec_memory[UNIWILL_EC_REG_CMDL] = 0x12; ec_memory[UNIWILL_EC_REG_CMDH] = 0x34;
    ec_count = ec_poll_count = release_errors = release_errno = timeout_errors = 0;
    ec_ready_at = 1; ec_fail_at = -1; ec_cleanup_error = 0;
    ec_fault_errno = -EIO; ec_triggered = ec_land_on_failure = false;
}

/* @DRIVER_FUNCTIONS@ */

struct bounded_response {
	u32 before;
	u32 words[10];
	u32 after;
};

static void assert_released(void)
{
	assert(!allocated_input && !allocated_output);
	assert(!uniwill_ec_lock && lock_calls == unlock_calls);
}

static void configure_error(unsigned int error)
{
	allocation_failure = missing_object = null_payload = false;
	acpi_result = 0;
	response_type = ACPI_TYPE_BUFFER;
	response_length = 40;
	switch (error) {
	case 0: allocation_failure = true; break;
	case 1: acpi_result = 1; break;
	case 2: missing_object = true; break;
	case 3: response_type = ACPI_TYPE_INTEGER; break;
	case 4: null_payload = true; break;
	case 5: response_length = 3; break;
	case 6: response_length = 41; break;
	default: assert(false);
	}
}

static void evaluate_error(int expected)
{
	struct bounded_response output;
	memset(&output, 0xa5, sizeof(output));
	struct bounded_response original = output;
	int calls_before = wmi_calls;
	assert(uw_wmi_ec_evaluate(0xab, 0x78563412, output.words) == expected);
	assert(memcmp(&output, &original, sizeof(output)) == 0);
	assert(wmi_calls == calls_before + !allocation_failure);
	assert_released();
}

int main(int argc, char **argv)
{
	assert(argc == 2);
	const char *test = argv[1];
	reset_ec();
	for (size_t i = 0; i < sizeof(payload); ++i)
		payload[i] = (u8)(i + 0x10);

	if (strcmp(test, "allocation_failure") == 0) {
		allocation_failure = true;
		evaluate_error(-ENOMEM);
		assert(allocation_calls == 1 && !lock_calls && !wmi_calls);
	} else if (strcmp(test, "null_destination") == 0) {
		assert(uw_wmi_ec_evaluate(0xab, 0x78563412, NULL) == -EINVAL);
		assert(!allocation_calls && !lock_calls && !wmi_calls);
	} else if (strcmp(test, "request_abi") == 0) {
		u32 output[10];
		const u8 expected[] = {0x12, 0x34, 0x56, 0x78, 0, 0xab, 0, 0};
		assert(uw_wmi_ec_evaluate(0xab, 0x78563412, output) == 0);
		assert(memcmp(request, expected, sizeof(expected)) == 0);
		assert(input_frees == 1 && output_frees == 1);
	} else if (strcmp(test, "acpi_failure_with_object") == 0) {
		acpi_result = 1;
		evaluate_error(-EIO);
		assert(input_frees == 1 && output_frees == 1);
	} else if (strcmp(test, "acpi_failure_without_object") == 0) {
		acpi_result = 1;
		missing_object = true;
		evaluate_error(-EIO);
		assert(input_frees == 1 && output_frees == 0);
	} else if (strcmp(test, "missing_object") == 0) {
		missing_object = true;
		evaluate_error(-EIO);
	} else if (strcmp(test, "wrong_type") == 0) {
		response_type = ACPI_TYPE_INTEGER;
		evaluate_error(-EIO);
	} else if (strcmp(test, "null_payload") == 0) {
		null_payload = true;
		evaluate_error(-EIO);
	} else if (strcmp(test, "short_payload") == 0) {
		for (response_length = 0; response_length < 4; ++response_length)
			evaluate_error(-EIO);
	} else if (strcmp(test, "oversized_payload") == 0) {
		response_length = 41;
		evaluate_error(-EIO);
		response_length = UINT32_MAX;
		evaluate_error(-EIO);
	} else if (strcmp(test, "valid_lengths") == 0) {
		for (response_length = 4; response_length <= 40; ++response_length) {
			struct bounded_response output;
			memset(&output, 0xa5, sizeof(output));
			assert(uw_wmi_ec_evaluate(0xab, 0x78563412, output.words) == 0);
			assert(output.before == 0xa5a5a5a5 && output.after == 0xa5a5a5a5);
			assert(memcmp(output.words, payload, response_length) == 0);
			for (size_t i = response_length; i < sizeof(output.words); ++i)
				assert(((u8 *)output.words)[i] == 0xa5);
			assert_released();
		}
	} else if (strcmp(test, "helper_errors") == 0) {
		for (unsigned int i = 0; i < 7; ++i) {
			configure_error(i);
			int expected = i == 0 ? -ENOMEM : -EIO;
			union uw_ec_read_return read_output = {.dword = 0xdeadbeef};
			union uw_ec_write_return write_output = {.dword = 0xdeadbeef};
			assert(uw_ec_read_addr_wmi(0x34, 0x12, &read_output) == expected);
			assert(uw_ec_write_addr_wmi(0x34, 0x12, 0x56, 0x78, &write_output) == expected);
			assert(read_output.dword == 0xdeadbeef && write_output.dword == 0xdeadbeef);
			assert_released();
		}
	} else if (strcmp(test, "sentinel") == 0) {
		memset(payload, 0xfe, 4);
		union uw_ec_read_return read_output = {.dword = 0xdeadbeef};
		union uw_ec_write_return write_output = {.dword = 0xdeadbeef};
		assert(uw_ec_read_addr_wmi(0x34, 0x12, &read_output) == -EIO);
		assert(uw_ec_write_addr_wmi(0x34, 0x12, 0x56, 0x78, &write_output) == -EIO);
		assert(read_output.dword == 0xdeadbeef && write_output.dword == 0xdeadbeef);
	} else if (strcmp(test, "helper_success") == 0) {
		union uw_ec_read_return read_output;
		union uw_ec_write_return write_output;
		u32 expected;
		memcpy(&expected, payload, sizeof(expected));
		assert(uw_ec_read_addr_wmi(0x34, 0x12, &read_output) == 0);
		const u8 read_request[] = {0x34, 0x12, 0, 0, 0, UNIWILL_WMI_FUNCTION_READ, 0, 0};
		assert(memcmp(request, read_request, sizeof(request)) == 0);
		assert(read_output.dword == expected);
		assert(uw_ec_write_addr_wmi(0x34, 0x12, 0x56, 0x78, &write_output) == 0);
		const u8 write_request[] = {0x34, 0x12, 0x56, 0x78, 0, UNIWILL_WMI_FUNCTION_WRITE, 0, 0};
		assert(memcmp(request, write_request, sizeof(request)) == 0);
		assert(write_output.dword == expected);
	} else if (strcmp(test, "ram_read_error") == 0) {
		for (unsigned int i = 0; i < 7; ++i) {
			configure_error(i);
			u8 output = 0xa5;
			assert(uw_wmi_read_ec_ram(0x1234, &output) == (i == 0 ? -ENOMEM : -EIO));
			assert(output == 0xa5);
			assert_released();
		}
		configure_error(1);
		acpi_result = 0;
		memset(payload, 0xfe, 4);
		u8 output = 0xa5;
		assert(uw_wmi_read_ec_ram(0x1234, &output) == -EIO && output == 0xa5);
	} else if (strcmp(test, "direct_ram_read_error") == 0) {
		uniwill_ec_direct = true;
		ec_fail_at = 0; ec_fault_errno = -ETIMEDOUT;
		u8 output = 0xa5;
		assert(uw_wmi_read_ec_ram(0x1234, &output) == -ETIMEDOUT);
		assert(output == 0xa5 && ec_count == 1 && !wmi_calls);
	} else if (strcmp(test, "ram_read_success") == 0) {
		u8 output = 0xa5;
		assert(uw_wmi_read_ec_ram(0x1234, &output) == 0 && output == payload[0]);
		uniwill_ec_direct = true;
		assert(uw_wmi_read_ec_ram(0x1234, &output) == 0 && output == 0x12);
		assert(ec_count == 9 && wmi_calls == 1);
		assert(uw_wmi_read_ec_ram(0x1234, NULL) == -EINVAL);
		assert(ec_count == 9 && wmi_calls == 1);
	} else if (strcmp(test, "ram_write_error") == 0) {
		acpi_result = 1;
		assert(uw_wmi_write_ec_ram(0x1234, 0x56) == -EIO);
		uniwill_ec_direct = true;
		ec_fail_at = 0; ec_fault_errno = -ETIMEDOUT;
		assert(uw_wmi_write_ec_ram(0x1234, 0x56) == -ETIMEDOUT);
		assert(ec_count == 1 && wmi_calls == 1);
    } else if (strcmp(test, "direct_success") == 0) {
        const u8 read_regs[] = {0x8c,0x8c,0x8a,0x8b,0x8c,0x8c,0x8d,0x8e,0x8c};
        const char read_ops[] = "rwwwwrrrw";
        const u8 write_regs[] = {0x8c,0x8c,0x8a,0x8b,0x8d,0x8e,0x8c,0x8c,0x8c};
        const char write_ops[] = "rwwwwwwrw";
        const u8 read_values[] = {0x20,0x24,0x34,0x12,0x25,0xa5,0x12,0x34,0};
        const u8 write_values[] = {0x20,0x24,0x34,0x12,0x56,0x78,0x26,0xa6,0};
        for (int write = 0; write < 2; ++write) {
            for (int busy = 0; busy < 2; ++busy) {
                reset_ec(); if (busy) ec_memory[0x8c] |= 4;
                union uw_ec_read_return ro = {.dword=0xdeadbeef};
                union uw_ec_write_return wo = {.dword=0xdeadbeef};
                int result = write ? uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,&wo) : uw_ec_read_addr_direct(0x34,0x12,&ro);
                assert(result == 0 && ec_count == 9 && ec_poll_count == 1);
                assert((write ? wo.dword : ro.dword) == (write ? 0x78561234u : 0x00003412u));
                for (int i=0; i<9; ++i) {
                    assert(ec_events[i].reg == (write ? write_regs[i] : read_regs[i]));
                    assert(ec_events[i].op == (write ? write_ops[i] : read_ops[i]));
                    assert(ec_events[i].value == ((i==0 && busy) ? 0x24 : write ? write_values[i] : read_values[i]));
                }
                assert(ec_events[1].value == 0x24);
                assert(ec_events[write ? 6 : 4].value == (write ? 0x26 : 0x25));
                assert(ec_memory[0x8c] == 0); assert_released();
            }
        }
    } else if (strcmp(test, "direct_each_error") == 0) {
        const int codes[] = {-EIO,-ENODEV,-ETIMEDOUT};
        for (int write=0; write<2; ++write) for (int landed=0; landed<2; ++landed)
          for (unsigned int c=0;c<sizeof(codes)/sizeof(codes[0]);++c) for (int fail=0;fail<9;++fail) {
            reset_ec(); ec_fail_at=fail; ec_fault_errno=codes[c]; ec_land_on_failure=landed;
            union uw_ec_read_return ro = {.dword=0xdeadbeef};
            union uw_ec_write_return wo = {.dword=0xdeadbeef};
            int result = write ? uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,&wo) : uw_ec_read_addr_direct(0x34,0x12,&ro);
            assert(result==codes[c] && ro.dword==0xdeadbeef && wo.dword==0xdeadbeef);
            assert(ec_count == (fail==0 ? 1 : fail==8 ? 9 : fail+2));
            if (fail) { assert(ec_events[ec_count-1].op=='w' && ec_events[ec_count-1].reg==0x8c && ec_events[ec_count-1].value==0); }
            assert(release_errors == (fail==8)); assert_released();
          }
    } else if (strcmp(test, "direct_cleanup_errors") == 0) {
        for (int write=0;write<2;++write) for (int fail=1;fail<8;++fail) {
            reset_ec(); ec_fail_at=fail; ec_fault_errno=-ENODEV; ec_cleanup_error=-EACCES;
            union uw_ec_read_return ro={.dword=0xdeadbeef}; union uw_ec_write_return wo={.dword=0xdeadbeef};
            int result=write ? uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,&wo) : uw_ec_read_addr_direct(0x34,0x12,&ro);
            assert(result==-ENODEV && release_errors==1 && release_errno==-EACCES);
            assert(ro.dword==0xdeadbeef && wo.dword==0xdeadbeef); assert_released();
        }
    } else if (strcmp(test, "direct_last_poll_and_timeout") == 0) {
        for (int write=0;write<2;++write) for (int ready=1;ready>=0;--ready) {
            reset_ec(); ec_ready_at=ready ? UW_EC_BUSY_WAIT_CYCLES : 0;
            union uw_ec_read_return ro={.dword=0xdeadbeef}; union uw_ec_write_return wo={.dword=0xdeadbeef};
            int result=write ? uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,&wo) : uw_ec_read_addr_direct(0x34,0x12,&ro);
            assert(result==(ready ? 0 : -EIO) && ec_poll_count==UW_EC_BUSY_WAIT_CYCLES);
            assert(timeout_errors==!ready && ec_memory[0x8c]==0);
            if (!ready) assert(ro.dword==0xdeadbeef && wo.dword==0xdeadbeef);
            assert_released();
        }
    } else if (strcmp(test, "direct_poll_errors") == 0) {
        for (int write=0;write<2;++write) for (int poll=0;poll<UW_EC_BUSY_WAIT_CYCLES;++poll) {
            reset_ec(); ec_ready_at=0; ec_fail_at=(write ? 7 : 5)+poll; ec_fault_errno=-ETIMEDOUT;
            union uw_ec_read_return ro={.dword=0xdeadbeef}; union uw_ec_write_return wo={.dword=0xdeadbeef};
            int result=write ? uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,&wo) : uw_ec_read_addr_direct(0x34,0x12,&ro);
            assert(result==-ETIMEDOUT && !timeout_errors && ec_poll_count==poll+1);
            assert(ro.dword==0xdeadbeef && wo.dword==0xdeadbeef && ec_memory[0x8c]==0); assert_released();
        }
    } else if (strcmp(test, "direct_caller_guard") == 0) {
        for (int fail=0;fail<9;++fail) {
            reset_ec(); uniwill_ec_direct=true; ec_fail_at=fail; ec_fault_errno=-ENODEV;
            u8 value=0xa5; assert(uw_wmi_read_ec_ram(0x1234,&value)==-ENODEV && value==0xa5 && !wmi_calls);
        }
        reset_ec(); assert(uw_ec_read_addr_direct(0x34,0x12,NULL)==-EINVAL);
        assert(uw_ec_write_addr_direct(0x34,0x12,0x56,0x78,NULL)==-EINVAL);
        assert(!ec_count && !uniwill_ec_lock);

	} else {
		assert(false);
	}
	assert_released();
	return 0;
}
