/* Parsing is a service double; the actual sysfs functions validate the domain. */
#include <ctype.h>
#include <sys/types.h>
struct device { int unused; };
struct device_attribute { int unused; };
static int kstrtou8(const char *text, unsigned int base, u8 *value)
{
    if (!*text || isspace((unsigned char)*text) || *text == '-') return -EINVAL;
    char *end;
    errno = 0;
    unsigned long parsed = strtoul(text, &end, base);
    if (end == text || (*end && strcmp(end, "\n"))) return -EINVAL;
    if (errno == ERANGE || parsed > UINT8_MAX) return -ERANGE;
    *value = parsed;
    return 0;
}
/* @HARNESS@ */
enum operation { AC, WRITE, UPDATE };
struct request { enum operation operation; u8 input, bit, value; int status; };
static struct request requests[3];
static void *run(void *argument)
{
    actor = (int)(intptr_t)argument;
    struct request *r = &requests[actor];
    if (r->operation == AC) r->status = uw_set_ac_auto_boot(r->input);
    else if (r->operation == WRITE) r->status = uniwill_write_ec_ram(UW_EC_REG_AC_AUTO_BOOT_STATUS, r->value);
    else r->status = uniwill_update_ec_ram(UW_EC_REG_AC_AUTO_BOOT_STATUS, r->bit, r->bit);
    assert(lock_depth == 0);
    pthread_mutex_lock(&harness);
    completed[actor] = true;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    return NULL;
}
static void interleave(struct request first, struct request second)
{
    requests[1] = first; requests[2] = second;
    pause_kind = first.operation == WRITE ? 'w' : 'r';
    gate_read_unlock = true; /* Expose any read-unlock-write implementation. */
    pthread_t threads[2];
    assert(pthread_create(&threads[0], NULL, run, (void *)(intptr_t)1) == 0);
    struct timespec end; assert(clock_gettime(CLOCK_REALTIME, &end) == 0); end.tv_sec += 3;
    pthread_mutex_lock(&harness);
    while (!paused) assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    pthread_mutex_unlock(&harness);
    assert(pthread_create(&threads[1], NULL, run, (void *)(intptr_t)2) == 0);
    pthread_mutex_lock(&harness);
    while (!waiting[2] && !completed[2]) assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
    assert(waiting[2] && !completed[2] && event_count == 1);
    release_pause = true; pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&harness);
    assert(pthread_join(threads[0], NULL) == 0 && pthread_join(threads[1], NULL) == 0);
    assert(requests[1].status == 0 && requests[2].status == 0 && lock_calls == unlock_calls);
}
static void check_concurrent_value(u16 address, u8 expected)
{
    if (ec[address] != expected) {
        fprintf(stderr, "EC %#06x: expected %#04x, got %#04x; transport trace:\n", address, expected, ec[address]);
        for (int i = 0; i < event_count; ++i)
            fprintf(stderr, "  actor %d %c %#06x %#04x\n", events[i].actor, events[i].kind,
                    events[i].address, events[i].value);
    }
    assert(ec[address] == expected);
}
static const int errors[] = {-EIO, -ENODEV, -ETIMEDOUT, -EINVAL, -ENOMEM, 7};
int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *test = argv[1]; reset();
    const u16 address = UW_EC_REG_AC_AUTO_BOOT_STATUS;
    const u8 mask = 1 << 3;
    if (!strcmp(test, "set_bytes")) {
        for (unsigned int b = 0; b <= UINT8_MAX; ++b)
            for (unsigned int input = 0; input <= UINT8_MAX; ++input) {
                reset(); ec[address] = b;
                uw_ac_auto_boot_last_written_value = !(input & 1);
                assert(uw_set_ac_auto_boot(input) == 0);
                assert(ec[address] == (u8)((b & ~mask) | ((input & 1) << 3)));
                assert(uw_ac_auto_boot_last_written_value == !!(input & 1));
                assert(event_count == 2);
                check_event(0, 0, 'r', address, b);
                check_event(1, 0, 'w', address, ec[address]);
            }
    } else if (!strcmp(test, "get_bytes")) {
        for (unsigned int b = 0; b <= UINT8_MAX; ++b) {
            reset(); ec[address] = b; u8 output = 42;
            uw_ac_auto_boot_last_written_value = !(b & mask);
            assert(uw_get_ac_auto_boot(&output) == 0 && output == !!(b & mask));
            assert(uw_ac_auto_boot_last_written_value == !(b & mask));
            assert(event_count == 1); check_event(0, 0, 'r', address, b);
        }
    } else if (!strcmp(test, "read_errors") || !strcmp(test, "write_errors") || !strcmp(test, "partial_write")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i)
            for (unsigned int input = 0; input <= 1; ++input) {
                reset(); ec[address] = input ? 0xa5 : 0xad;
                bool old_cache = !input;
                uw_ac_auto_boot_last_written_value = old_cache;
                bool reading = !strcmp(test, "read_errors");
                if (reading) read_error = errors[i]; else write_error = errors[i];
                failed_write_applies = !strcmp(test, "partial_write");
                assert(uw_set_ac_auto_boot(input) == errors[i]);
                assert(event_count == (reading ? 1 : 2));
                assert(ec[address] == (failed_write_applies ? (input ? 0xad : 0xa5) : (input ? 0xa5 : 0xad)));
                assert(uw_ac_auto_boot_last_written_value == old_cache);
                check_event(0, 0, 'r', address, input ? 0xa5 : 0xad);
                if (!reading) check_event(1, 0, 'w', address, input ? 0xad : 0xa5);
                read_error = write_error = 0; event_count = 0;
                assert(uw_set_ac_auto_boot(input) == 0);
                assert(ec[address] == (input ? 0xad : 0xa5) && uw_ac_auto_boot_last_written_value == !!input);
                assert(event_count == 2); /* Error paths must release the transaction lock. */
            }
    } else if (!strcmp(test, "get_errors")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); read_error = errors[i]; u8 output = 42;
            uw_ac_auto_boot_last_written_value = true;
            assert(uw_get_ac_auto_boot(&output) == errors[i] && output == 42 && event_count == 1);
            assert(uw_ac_auto_boot_last_written_value);
        }
        reset(); uw_ac_auto_boot_last_written_value = true;
        assert(uw_get_ac_auto_boot(NULL) == -EINVAL && event_count == 1);
        assert(uw_ac_auto_boot_last_written_value);
    } else if (!strcmp(test, "missing")) {
        struct uniwill_interface_t *missing[] = {NULL, (void *)(intptr_t)-ENODEV};
        for (unsigned int i = 0; i < sizeof(missing) / sizeof(missing[0]); ++i)
            for (unsigned int input = 0; input <= 1; ++input) {
                reset(); uniwill_interfaces.wmi = missing[i]; u8 output = 42;
                uw_ac_auto_boot_last_written_value = !input;
                assert(uw_set_ac_auto_boot(input) == -EIO);
                assert(uw_get_ac_auto_boot(&output) == -EIO && output == 42 && !event_count);
                assert(uw_ac_auto_boot_last_written_value == !input);
            }
    } else if (!strcmp(test, "sysfs")) {
        const char *invalid[] = {"", "2", "255", "256", "-1", "-0", "abc", "1x", "1\n0", " 1", "+"};
        for (unsigned int i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i) {
            reset(); uw_ac_auto_boot_last_written_value = true;
            assert(uw_ac_auto_boot_store(NULL, NULL, invalid[i], strlen(invalid[i])) == -EINVAL && !event_count);
            assert(uw_ac_auto_boot_last_written_value);
        }
        const char *valid[] = {"0", "0\n", "1", "+1\n"};
        for (unsigned int i = 0; i < sizeof(valid) / sizeof(valid[0]); ++i) {
            reset(); ec[address] = 0xa5;
            bool on = valid[i][0] != '0'; uw_ac_auto_boot_last_written_value = !on;
            size_t size = strlen(valid[i]);
            assert(uw_ac_auto_boot_store(NULL, NULL, valid[i], size) == (ssize_t)size);
            assert(ec[address] == (on ? 0xad : 0xa5) && uw_ac_auto_boot_last_written_value == on);
            char output[8];
            assert(uw_ac_auto_boot_show(NULL, NULL, output) == 2);
            assert(!strcmp(output, on ? "1\n" : "0\n") && event_count == 3);
        }
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); char output[8] = "kept"; read_error = errors[i];
            uw_ac_auto_boot_last_written_value = false;
            assert(uw_ac_auto_boot_show(NULL, NULL, output) == -EIO && !strcmp(output, "kept"));
            assert(uw_ac_auto_boot_store(NULL, NULL, "1", 1) == -EIO && event_count == 2);
            assert(!uw_ac_auto_boot_last_written_value);
            reset(); write_error = errors[i]; uw_ac_auto_boot_last_written_value = false;
            assert(uw_ac_auto_boot_store(NULL, NULL, "1", 1) == -EIO && event_count == 2);
            assert(!uw_ac_auto_boot_last_written_value);
        }
    } else if (!strcmp(test, "ac_update") || !strcmp(test, "update_ac")) {
        for (unsigned int bit = 1; bit <= 0x80; bit <<= 1) {
            if (bit == mask) continue;
            for (unsigned int input = 0; input <= 1; ++input) {
                reset(); ec[address] = input ? 0 : mask;
                struct request ac = {.operation=AC, .input=input};
                struct request other = {.operation=UPDATE, .bit=bit};
                bool first = !strcmp(test, "ac_update");
                interleave(first ? ac : other, first ? other : ac);
                check_concurrent_value(address, bit | (input ? mask : 0));
                assert(event_count == 4);
                check_event(0, 1, 'r', address, input ? 0 : mask);
                assert(events[1].actor == 1 && events[1].kind == 'w');
                assert(events[2].actor == 2 && events[2].kind == 'r');
                assert(events[3].actor == 2 && events[3].kind == 'w');
            }
        }
    } else if (!strcmp(test, "ac_write") || !strcmp(test, "write_ac")) {
        for (unsigned int input = 0; input <= 1; ++input) {
            reset(); ec[address] = 0xa5;
            struct request ac = {.operation=AC, .input=input};
            struct request write = {.operation=WRITE, .value=0xc3};
            bool first = !strcmp(test, "ac_write");
            interleave(first ? ac : write, first ? write : ac);
            check_concurrent_value(address, first ? 0xc3 : (0xc3 | (input ? mask : 0)));
            assert(event_count == 3);
            if (first) {
                check_event(0, 1, 'r', address, 0xa5); check_event(1, 1, 'w', address, input ? 0xad : 0xa5);
                check_event(2, 2, 'w', address, 0xc3);
            } else {
                check_event(0, 1, 'w', address, 0xc3); check_event(1, 2, 'r', address, 0xc3);
                check_event(2, 2, 'w', address, ec[address]);
            }
        }
    } else abort();
    assert(lock_depth == 0 && lock_calls == unlock_calls);
    return 0;
}
