/* Compile the actual production fan-table engine. Only EC transport is fake. */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

typedef uint8_t u8;
typedef uint16_t u16;
/* Linux reserves this token as a task accessor macro. Keep the real
 * expansion shape so userspace fixtures catch kernel-only name collisions. */
#define current get_current()
/* @ACTUAL_ENGINE_HEADER@ */

struct event { char kind; u16 address; u8 value; };
struct fault { char kind; unsigned ordinal; int error; bool landed, corrupt; };
struct hardware {
    u8 bytes[65536];
    struct event events[4096];
    unsigned event_count, reads, writes, updates;
    struct fault faults[3];
    unsigned fault_count;
    bool perturb_other_bits, perturb_reserved;
    unsigned perturb_enable;
};
static struct hardware hw;
static unsigned scenarios, assertions;
static const char *running_case;
static unsigned selected_position;

#define CHECK(condition) do { ++assertions; if (!(condition)) { \
    fprintf(stderr, "%s position=%u line=%d: %s\n", running_case, \
            selected_position, __LINE__, #condition); exit(1); } } while (0)

static void log_event(char kind, u16 address, u8 value)
{
    CHECK(hw.event_count < sizeof(hw.events) / sizeof(hw.events[0]));
    hw.events[hw.event_count++] = (struct event){kind, address, value};
}
static struct fault *fault_at(char kind, unsigned ordinal)
{
    for (unsigned i = 0; i < hw.fault_count; ++i)
        if (hw.faults[i].kind == kind && hw.faults[i].ordinal == ordinal)
            return &hw.faults[i];
    return NULL;
}
static int ec_read(void *context, u16 address, u8 *value)
{
    CHECK(context == &hw);
    struct fault *fault = fault_at('r', ++hw.reads);
    log_event('r', address, hw.bytes[address]);
    if (fault && !fault->corrupt) {
        *value = 0xff; /* A failed read may leave an unusable output value. */
        return fault->error;
    }
    *value = hw.bytes[address];
    if (fault && fault->corrupt)
        *value ^= address == 0x07c6 ? 0x04 : address == 0x07c5 ? 0x80 : address == 0x0751 ? 0x40 : 0x01;
    return 0;
}
static int store(char kind, u16 address, u8 value)
{
    struct fault *fault = fault_at('w', ++hw.writes);
    log_event(kind, address, value);
    if (!fault || fault->landed)
        hw.bytes[address] = value;
    if (hw.perturb_reserved && address >= 0x0f00 && address <= 0x0f5f) {
        hw.bytes[0x0f10] ^= 0x01;
        hw.perturb_reserved = false;
    }
    if (hw.perturb_enable && address == 0x07c6 && (value & 4)) {
        unsigned mutation = hw.perturb_enable;
        hw.perturb_enable = 0;
        if (mutation == 1) hw.bytes[0x0f21] ^= 1;
        else if (mutation == 2) hw.bytes[0x07c5] ^= 0x80;
        else if (mutation == 3) { hw.bytes[0x07c5] ^= 1; hw.bytes[0x07c6] ^= 0x80; }
    }
    return fault ? fault->error : 0;
}
static int ec_write(void *context, u16 address, u8 value)
{
    CHECK(context == &hw);
    return store('w', address, value);
}
static int ec_update(void *context, u16 address, u8 clear, u8 set)
{
    CHECK(context == &hw);
    ++hw.updates;
    u8 owned = address == 0x07c5 ? 0x80 : address == 0x07c6 ? 0x04 : 0;
    CHECK(owned != 0);
    CHECK(((clear | set) & (u8)~owned) == 0);
    if (hw.perturb_other_bits)
        hw.bytes[address] ^= (u8)~owned;
    return store('u', address, (hw.bytes[address] & (u8)~clear) | set);
}
static struct uw_curve_io transport(void)
{
    return (struct uw_curve_io){.ctx = &hw, .read = ec_read, .write = ec_write,
                                .update_bits = ec_update};
}
static void reset(void)
{
    ++scenarios;
    memset(&hw, 0, sizeof(hw));
    for (unsigned i = 0; i < 96; ++i)
        hw.bytes[0x0f00 + i] = (u8)(11 + i * 37);
    const u8 duty[16] = {0, 1, 3, 99, 127, 199, 200, 255,
                         255, 127, 3, 1, 0, 255, 255, 199};
    memcpy(&hw.bytes[0x0f20], duty, 16);
    memcpy(&hw.bytes[0x0f50], duty, 16);
    hw.bytes[0x0f10] = 0x31;
    hw.bytes[0x0f40] = 0xc7;
    hw.bytes[0x07c5] = 0x35;
    hw.bytes[0x07c6] = 0xad; /* Initial custom-table enable bit is set. */
    hw.bytes[0x0751] = 0x12;
}
static struct uw_curve_input input(void)
{
    struct uw_curve_input value = {0};
    for (unsigned fan = 0; fan < 2; ++fan) {
        for (unsigned i = 0; i < 15; ++i) {
            value.rise[fan][i] = i < 6 ? (u8)(20 + i * 7 + fan) : 255;
            value.fall[fan][i] = i < 6 ? (u8)(10 + i * 7 + fan) : 255;
        }
        for (unsigned i = 0; i < 16; ++i)
            value.duty_raw[fan][i] = i < 8 ? (u8)(i * 3) : 200;
    }
    value.respective = 1;
    return value;
}
static void raw_equal(const struct uw_curve_raw *a, const struct uw_curve_raw *b)
{
    CHECK(a->valid && b->valid);
    CHECK(memcmp(a->table, b->table, 96) == 0);
    CHECK(a->ctrl7c5 == b->ctrl7c5);
    CHECK(a->ctrl7c6 == b->ctrl7c6);
    CHECK(a->mode751 == b->mode751);
}
static void restored(const struct uw_curve_raw *before)
{
    CHECK(before->valid);
    CHECK(memcmp(&hw.bytes[0x0f00], before->table, 96) == 0);
    CHECK(hw.bytes[0x07c5] == before->ctrl7c5);
    CHECK(hw.bytes[0x07c6] == before->ctrl7c6);
    CHECK(hw.bytes[0x0751] == before->mode751);
}
static void fault(char kind, unsigned ordinal, int error, bool landed, bool corrupt)
{
    CHECK(hw.fault_count < 3);
    hw.faults[hw.fault_count++] = (struct fault){kind, ordinal, error, landed, corrupt};
}
static void no_reserved_writes(void)
{
    for (unsigned i = 0; i < hw.event_count; ++i)
        if (hw.events[i].kind != 'r')
            CHECK(hw.events[i].address != 0x0f10 && hw.events[i].address != 0x0f40);
}

static void snapshot_failures(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_raw raw = {0};
    CHECK(uw_curve_snapshot(&io, &raw) == 0);
    CHECK(raw.valid && hw.writes == 0);
    unsigned count = hw.reads;
    CHECK(count == 99);
    for (unsigned position = 1; position <= count; ++position) {
        selected_position = position;
        reset(); io = transport(); raw = (struct uw_curve_raw){0};
        fault('r', position, -EREMOTEIO, false, false);
        CHECK(uw_curve_snapshot(&io, &raw) == -EREMOTEIO);
        CHECK(!raw.valid && hw.writes == 0);
        reset(); io = transport();
        struct uw_curve_input request = input();
        struct uw_curve_result result = {0};
        fault('r', position, -EREMOTEIO, false, false);
        CHECK(uw_curve_commit(&io, &request, &raw, &result) == -EREMOTEIO);
        CHECK(!raw.valid && !result.snapshot_valid && !result.write_attempted);
        CHECK(!result.applied_verified && !result.restored_verified && hw.writes == 0);
    }
    /* The live FULL check and disable decision happen after a valid snapshot
     * but still before the first mutation. Those read failures also write zero. */
    reset(); io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &raw, &result) == 0);
    unsigned reads_before_first_store = 0;
    for (unsigned i = 0; i < hw.event_count && hw.events[i].kind == 'r'; ++i)
        ++reads_before_first_store;
    CHECK(reads_before_first_store > count);
    for (unsigned position = count + 1; position <= reads_before_first_store; ++position) {
        selected_position = position;
        reset(); io = transport();
        fault('r', position, -EREMOTEIO, false, false);
        CHECK(uw_curve_commit(&io, &request, &raw, &result) == -EREMOTEIO);
        CHECK(raw.valid && result.snapshot_valid && !result.write_attempted);
        CHECK(!result.applied_verified && !result.restored_verified && hw.writes == 0);
    }
}
static void successful_order(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    CHECK(result.snapshot_valid && result.write_attempted && result.applied_verified);
    CHECK(!result.restored_verified && result.original_errno == 0 && result.restore_errno == 0);
    unsigned table_writes = 0, seen[96] = {0};
    bool disabled = false, enabled = false;
    for (unsigned i = 0; i < hw.event_count; ++i) {
        struct event event = hw.events[i];
        if (event.kind == 'r') continue;
        if (event.address == 0x07c6) {
            if (!(event.value & 4)) disabled = true;
            else { CHECK(disabled && table_writes == 94); enabled = true; }
        }
        if (event.address >= 0x0f00 && event.address <= 0x0f5f) {
            CHECK(disabled && !enabled);
            CHECK(++seen[event.address - 0x0f00] == 1);
            ++table_writes;
        }
    }
    CHECK(table_writes == 94 && enabled);
    for (unsigned i = 0; i < 96; ++i)
        CHECK(seen[i] == (i == 0x10 || i == 0x40 ? 0u : 1u));
    for (unsigned fan = 0; fan < 2; ++fan) {
        u16 base = (u16)(0x0f00 + fan * 0x30);
        for (unsigned i = 0; i < 16; ++i) {
            CHECK(hw.bytes[base + i] == (i == 15 ? 255 : request.rise[fan][i]));
            if (i < 15) CHECK(hw.bytes[base + 0x11 + i] == request.fall[fan][i]);
            CHECK(hw.bytes[base + 0x20 + i] == request.duty_raw[fan][i]);
        }
    }
    CHECK(hw.bytes[0x0f10] == before.table[0x10]);
    CHECK(hw.bytes[0x0f40] == before.table[0x40]);
    no_reserved_writes();
    struct uw_curve_result recovery = {0};
    CHECK(uw_curve_restore(&io, &before, &recovery) == 0);
    CHECK(recovery.restored_verified && recovery.restore_errno == 0);
    restored(&before);
    no_reserved_writes();
}
static void commit_write_failures(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    unsigned count = hw.writes;
    CHECK(count >= 94);
    for (unsigned landed = 0; landed < 2; ++landed) {
        for (unsigned position = 1; position <= count; ++position) {
            selected_position = position;
            reset(); io = transport(); before = (struct uw_curve_raw){0};
            result = (struct uw_curve_result){0};
            fault('w', position, -EREMOTEIO, landed != 0, false);
            CHECK(uw_curve_commit(&io, &request, &before, &result) == -EREMOTEIO);
            CHECK(result.original_errno == -EREMOTEIO);
            CHECK(result.snapshot_valid && before.valid && result.write_attempted);
            CHECK(!result.applied_verified && result.restored_verified && result.restore_errno == 0);
            restored(&before);
            no_reserved_writes();
        }
    }
}
static void commit_read_failures(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    unsigned read_count = hw.reads, first_postwrite_read = 0, read_position = 0;
    bool has_written = false;
    for (unsigned i = 0; i < hw.event_count; ++i) {
        if (hw.events[i].kind != 'r') has_written = true;
        else if (++read_position && has_written && !first_postwrite_read)
            first_postwrite_read = read_position;
    }
    CHECK(first_postwrite_read > 99 && first_postwrite_read <= read_count);
    for (unsigned corrupt = 0; corrupt < 2; ++corrupt) {
        for (unsigned position = first_postwrite_read; position <= read_count; ++position) {
            selected_position = position;
            reset(); io = transport(); before = (struct uw_curve_raw){0};
            result = (struct uw_curve_result){0};
            fault('r', position, -EREMOTEIO, false, corrupt != 0);
            int status = uw_curve_commit(&io, &request, &before, &result);
            if (!corrupt) CHECK(status == -EREMOTEIO);
            if (status) {
                CHECK(result.original_errno < 0 && !result.applied_verified);
                CHECK(result.snapshot_valid && result.restored_verified && result.restore_errno == 0);
                restored(&before);
            } else {
                /* A changed preliminary observation may cause an extra safe
                 * RMW. Success must still match the whole intended state. */
                struct uw_curve_raw target;
                CHECK(corrupt && result.applied_verified);
                CHECK(uw_curve_build_raw(&before, &request, &target) == 0);
                CHECK(uw_curve_compare(&io, &target) == 0);
            }
        }
    }
}
static void restore_failures(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    /* Third store is after a successful disable and respective-bit update. */
    fault('w', 3, -EIO, false, false);
    CHECK(uw_curve_commit(&io, &request, &before, &result) == -EIO);
    CHECK(result.restored_verified);
    unsigned restore_writes = hw.writes - 3;
    CHECK(restore_writes >= 94);
    for (unsigned landed = 0; landed < 2; ++landed) {
        for (unsigned position = 1; position <= restore_writes; ++position) {
            selected_position = position;
            reset(); io = transport(); before = (struct uw_curve_raw){0};
            result = (struct uw_curve_result){0};
            fault('w', 3, -EIO, false, false);
            fault('w', 3 + position, -EREMOTEIO, landed != 0, false);
            CHECK(uw_curve_commit(&io, &request, &before, &result) == -EIO);
            CHECK(result.original_errno == -EIO && result.restore_errno == -EREMOTEIO);
            CHECK(before.valid && result.snapshot_valid && !result.restored_verified);
            CHECK(!result.applied_verified && !(hw.bytes[0x07c6] & 4));
            struct uw_curve_raw saved = before;
            hw.fault_count = 0;
            struct uw_curve_result retry = {0};
            CHECK(uw_curve_restore(&io, &before, &retry) == 0);
            CHECK(retry.restored_verified);
            raw_equal(&before, &saved);
            restored(&before);
            no_reserved_writes();
        }
    }
}

static void restore_read_failures(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    hw.event_count = hw.reads = hw.writes = hw.updates = 0;
    CHECK(uw_curve_restore(&io, &before, &result) == 0);
    unsigned count = hw.reads;
    CHECK(count > 190);
    for (unsigned corrupt = 0; corrupt < 2; ++corrupt) {
        for (unsigned position = 1; position <= count; ++position) {
            selected_position = position;
            reset(); io = transport(); before = (struct uw_curve_raw){0};
            CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
            hw.event_count = hw.reads = hw.writes = hw.updates = 0;
            fault('r', position, -EREMOTEIO, false, corrupt != 0);
            int status = uw_curve_restore(&io, &before, &result);
            if (!corrupt) CHECK(status == -EREMOTEIO);
            CHECK(before.valid && result.snapshot_valid);
            if (status) {
                CHECK(!result.restored_verified && result.restore_errno == status);
                if (hw.writes) CHECK(!(hw.bytes[0x07c6] & 4));
            } else {
                CHECK(corrupt && result.restored_verified);
                restored(&before);
            }
            struct uw_curve_raw saved = before;
            hw.fault_count = 0;
            CHECK(uw_curve_restore(&io, &before, &result) == 0);
            CHECK(result.restored_verified);
            raw_equal(&before, &saved);
            restored(&before);
            no_reserved_writes();
        }
    }
}
static void enable_side_effects(void)
{
    for (unsigned mutation = 1; mutation <= 3; ++mutation) {
        selected_position = mutation;
        reset();
        struct uw_curve_io io = transport();
        struct uw_curve_input request = input();
        struct uw_curve_raw before = {0};
        struct uw_curve_result result = {0};
        hw.perturb_enable = mutation;
        int status = uw_curve_commit(&io, &request, &before, &result);
        if (mutation < 3) {
            CHECK(status < 0 && !result.applied_verified && result.restored_verified);
            restored(&before);
        } else {
            CHECK(status == 0 && result.applied_verified);
            CHECK((hw.bytes[0x07c5] & 0x7f) == ((before.ctrl7c5 ^ 1) & 0x7f));
            CHECK((hw.bytes[0x07c6] & 0xfb) == ((before.ctrl7c6 ^ 0x80) & 0xfb));
        }
        reset(); io = transport(); before = (struct uw_curve_raw){0};
        CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
        hw.perturb_enable = mutation;
        status = uw_curve_restore(&io, &before, &result);
        if (mutation < 3) {
            CHECK(status < 0 && !result.restored_verified && result.restore_errno < 0);
            CHECK(!(hw.bytes[0x07c6] & 4));
            CHECK(uw_curve_restore(&io, &before, &result) == 0);
            restored(&before);
        } else {
            CHECK(status == 0 && result.restored_verified);
            CHECK(memcmp(&hw.bytes[0x0f00], before.table, 96) == 0);
            CHECK((hw.bytes[0x07c5] & 0x80) == (before.ctrl7c5 & 0x80));
            CHECK((hw.bytes[0x07c6] & 4) == (before.ctrl7c6 & 4));
        }
        no_reserved_writes();
    }
}
static void read_only_and_encoding(void)
{
    reset();
    struct uw_curve_io io = transport();
    io.write = NULL; io.update_bits = NULL;
    struct uw_curve_raw before = {0}, target = {0};
    CHECK(uw_curve_snapshot(&io, &before) == 0);
    CHECK(uw_curve_compare(&io, &before) == 0);
    unsigned original_reads = hw.reads;
    struct uw_curve_input request = input();
    CHECK(uw_curve_build_raw(&before, &request, &target) == 0);
    CHECK(hw.reads == original_reads && hw.writes == 0);
    struct uw_curve_raw alias = before;
    CHECK(uw_curve_build_raw(&alias, &request, &alias) == 0);
    raw_equal(&alias, &target);
    CHECK(target.table[0x10] == before.table[0x10] && target.table[0x40] == before.table[0x40]);
    for (unsigned i = 0; i < 96; ++i) {
        hw.event_count = 0;
        hw.bytes[0x0f00 + i] ^= 1;
        CHECK(uw_curve_compare(&io, &before) == -EAGAIN);
        hw.bytes[0x0f00 + i] ^= 1;
    }
    hw.bytes[0x07c5] ^= 0x7f; hw.bytes[0x07c6] ^= 0xfb;
    CHECK(uw_curve_compare(&io, &before) == 0);
    hw.bytes[0x07c5] ^= 0x80;
    CHECK(uw_curve_compare(&io, &before) == -EAGAIN);
    hw.bytes[0x07c5] ^= 0x80; hw.bytes[0x07c6] ^= 4;
    CHECK(uw_curve_compare(&io, &before) == -EAGAIN);
    hw.bytes[0x07c6] ^= 4; hw.bytes[0x0751] ^= 1;
    CHECK(uw_curve_compare(&io, &before) == -EAGAIN);
}


static void landed_enable_failed_restore_entry(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    unsigned enable_write = hw.writes, reads_before_enable = 0;
    for (unsigned i = 0, writes = 0, reads = 0; i < hw.event_count; ++i) {
        if (hw.events[i].kind == 'r') ++reads;
        else if (++writes == enable_write) reads_before_enable = reads;
    }
    CHECK(enable_write >= 94 && reads_before_enable > 99);
    for (unsigned emergency_fault = 0; emergency_fault < 3; ++emergency_fault) {
        selected_position = emergency_fault;
        reset(); io = transport(); before = (struct uw_curve_raw){0};
        /* The enable reached firmware although its callback reports failure.
         * The first rollback read then fails before normal disable can run. */
        fault('w', enable_write, -EIO, true, false);
        fault('r', reads_before_enable + 1, -EREMOTEIO, false, false);
        if (emergency_fault == 1)
            fault('w', enable_write + 1, -EPIPE, false, false);
        else if (emergency_fault == 2)
            fault('r', reads_before_enable + 2, -EPIPE, false, false);
        CHECK(uw_curve_commit(&io, &request, &before, &result) == -EIO);
        CHECK(result.original_errno == -EIO && result.restore_errno == -EREMOTEIO);
        CHECK(result.snapshot_valid && before.valid && !result.restored_verified);
        CHECK(!result.applied_verified && hw.writes == enable_write + 1);
        struct event final_store = {0};
        for (unsigned i = 0; i < hw.event_count; ++i)
            if (hw.events[i].kind != 'r') final_store = hw.events[i];
        CHECK(final_store.kind == 'u' && final_store.address == 0x07c6 && !(final_store.value & 4));
        if (emergency_fault == 1) CHECK(hw.bytes[0x07c6] & 4);
        else CHECK(!(hw.bytes[0x07c6] & 4));
        hw.fault_count = 0;
        CHECK(uw_curve_restore(&io, &before, &result) == 0);
        restored(&before);
    }
}

static void invalid_inputs(void)
{
    for (unsigned fan = 0; fan < 2; ++fan) {
        for (unsigned index = 0; index < 16; ++index) {
            selected_position = fan * 16 + index;
            reset();
            struct uw_curve_io io = transport();
            struct uw_curve_input request = input();
            request.duty_raw[fan][index] = 201;
            struct uw_curve_raw before = {0};
            struct uw_curve_result result = {0};
            CHECK(uw_curve_commit(&io, &request, &before, &result) == -EINVAL);
            CHECK(hw.reads == 0 && hw.writes == 0 && !result.write_attempted);
        }
    }
    for (unsigned variant = 0; variant < 2; ++variant) {
        reset();
        struct uw_curve_io io = transport();
        struct uw_curve_input request = input();
        if (variant == 0) request.respective = 2;
        else request.rise[0][0] = request.rise[0][1] = 0;
        struct uw_curve_raw before = {0};
        struct uw_curve_result result = {0};
        CHECK(uw_curve_commit(&io, &request, &before, &result) == -EINVAL);
        CHECK(hw.reads == 0 && hw.writes == 0);
    }
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    request.rise[0][0] = 30; request.rise[0][1] = 20;
    request.fall[0][0] = 250; /* No invented monotonicity/Down<=Up rule. */
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
    reset(); io = transport();
    hw.bytes[0x0751] |= 0x40;
    CHECK(uw_curve_commit(&io, &request, &before, &result) == -EBUSY);
    CHECK(hw.writes == 0);
}
static void rmw_all_bytes(void)
{
    for (unsigned control = 0; control < 2; ++control) {
        for (unsigned byte = 0; byte < 256; ++byte) {
            selected_position = byte;
            reset();
            u16 address = control == 0 ? 0x07c5 : 0x07c6;
            u8 owned = control == 0 ? 0x80 : 0x04;
            hw.bytes[address] = (u8)byte;
            struct uw_curve_io io = transport();
            struct uw_curve_input request = input();
            request.respective = (u8)(byte & 1);
            struct uw_curve_raw before = {0};
            struct uw_curve_result result = {0};
            CHECK(uw_curve_commit(&io, &request, &before, &result) == 0);
            CHECK((hw.bytes[address] & (u8)~owned) == (byte & (u8)~owned));
            CHECK(hw.bytes[0x0751] == before.mode751);
            struct uw_curve_result recovery = {0};
            CHECK(uw_curve_restore(&io, &before, &recovery) == 0);
            restored(&before);
        }
    }
}
static void reserved_change(void)
{
    reset();
    struct uw_curve_io io = transport();
    struct uw_curve_input request = input();
    struct uw_curve_raw before = {0};
    struct uw_curve_result result = {0};
    hw.perturb_reserved = true;
    CHECK(uw_curve_commit(&io, &request, &before, &result) < 0);
    CHECK(result.snapshot_valid && before.valid && !result.applied_verified);
    CHECK(result.restore_errno != 0 && !result.restored_verified);
    CHECK(!(hw.bytes[0x07c6] & 4));
    CHECK(hw.bytes[0x0f10] != before.table[0x10]);
    no_reserved_writes();
    hw.bytes[0x0f10] = before.table[0x10];
    struct uw_curve_result retry = {0};
    CHECK(uw_curve_restore(&io, &before, &retry) == 0);
    restored(&before);
}
int main(int argc, char **argv)
{
    CHECK(argc == 2);
    running_case = argv[1];
    if (!strcmp(argv[1], "snapshot_failures")) snapshot_failures();
    else if (!strcmp(argv[1], "successful_order")) successful_order();
    else if (!strcmp(argv[1], "commit_write_failures")) commit_write_failures();
    else if (!strcmp(argv[1], "commit_read_failures")) commit_read_failures();
    else if (!strcmp(argv[1], "restore_failures")) restore_failures();
    else if (!strcmp(argv[1], "restore_read_failures")) restore_read_failures();
    else if (!strcmp(argv[1], "enable_side_effects")) enable_side_effects();
    else if (!strcmp(argv[1], "read_only_and_encoding")) read_only_and_encoding();
    else if (!strcmp(argv[1], "landed_enable_failed_restore_entry")) landed_enable_failed_restore_entry();
    else if (!strcmp(argv[1], "invalid_inputs")) invalid_inputs();
    else if (!strcmp(argv[1], "rmw_all_bytes")) rmw_all_bytes();
    else if (!strcmp(argv[1], "reserved_change")) reserved_change();
    else CHECK(false);
    printf("{\"case\":\"%s\",\"scenarios\":%u,\"assertions\":%u}\n",
           running_case, scenarios, assertions);
    return 0;
}
