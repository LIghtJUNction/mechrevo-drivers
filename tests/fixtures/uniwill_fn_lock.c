/* Parsing is a service double; the actual sysfs functions validate the domain. */
#include <ctype.h>
#include <sys/types.h>
struct device { int unused; };
struct device_attribute { int unused; };
static int kstrtoint(const char *text, unsigned int base, int *value)
{
    if (!*text || isspace((unsigned char)*text)) return -EINVAL;
    char *end;
    errno = 0;
    long parsed = strtol(text, &end, base);
    if (end == text || (*end && strcmp(end, "\n"))) return -EINVAL;
    if (errno == ERANGE || parsed < INT_MIN || parsed > INT_MAX) return -ERANGE;
    *value = parsed;
    return 0;
}
/* @HARNESS@ */
enum operation { FN, WRITE, UPDATE };
struct request { enum operation operation; int on, status; u8 bit, value; };
static struct request requests[3];
static void *run(void *argument)
{
    actor = (int)(intptr_t)argument;
    struct request *r = &requests[actor];
    if (r->operation == FN) r->status = uniwill_wmi_fn_lock_set(r->on);
    else if (r->operation == WRITE) r->status = uniwill_write_ec_ram(UW_EC_REG_KBD_FN_LOCK_STATUS_BIT, r->value);
    else r->status = uniwill_update_ec_ram(UW_EC_REG_KBD_FN_LOCK_STATUS_BIT, r->bit, r->bit);
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
static const int errors[] = {-EIO, -ENODEV, -ETIMEDOUT, -EINVAL, -ENOMEM, 7};
int main(int argc, char **argv)
{
    assert(argc == 2);
    const char *test = argv[1]; reset();
    const u16 address = UW_EC_REG_KBD_FN_LOCK_STATUS_BIT;
    if (!strcmp(test, "set_bytes")) {
        const int inputs[] = {0, 1, 2, -1, INT_MIN, INT_MAX};
        for (unsigned int b = 0; b <= UINT8_MAX; ++b)
            for (unsigned int i = 0; i < sizeof(inputs) / sizeof(inputs[0]); ++i) {
                reset(); ec[address] = b;
                assert(uniwill_wmi_fn_lock_set(inputs[i]) == 0);
                assert(ec[address] == (u8)((b & ~UNIWILL_FN_LOCK_MASK) | (inputs[i] ? UNIWILL_FN_LOCK_MASK : 0)));
                assert(event_count == 2);
                check_event(0, 0, 'r', address, b);
                check_event(1, 0, 'w', address, ec[address]);
            }
    } else if (!strcmp(test, "get_bytes")) {
        for (unsigned int b = 0; b <= UINT8_MAX; ++b) {
            reset(); ec[address] = b; int output = -1;
            assert(uniwill_wmi_fn_lock_get(&output) == 0 && output == !!(b & UNIWILL_FN_LOCK_MASK));
            assert(uniwill_wmi_fn_lock_get(NULL) == 0 && event_count == 2);
        }
    } else if (!strcmp(test, "read_errors") || !strcmp(test, "write_errors") || !strcmp(test, "partial_write")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); ec[address] = 0xa5;
            bool reading = !strcmp(test, "read_errors");
            if (reading) read_error = errors[i]; else write_error = errors[i];
            failed_write_applies = !strcmp(test, "partial_write");
            assert(uniwill_wmi_fn_lock_set(1) == errors[i]);
            assert(event_count == (reading ? 1 : 2));
            assert(ec[address] == (failed_write_applies ? 0xb5 : 0xa5));
            read_error = write_error = 0; event_count = 0;
            assert(uniwill_wmi_fn_lock_set(0) == 0 && ec[address] == 0xa5); /* also checks lock release */
        }
    } else if (!strcmp(test, "get_errors")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); read_error = errors[i]; int output = 42;
            assert(uniwill_wmi_fn_lock_get(&output) == errors[i] && output == 42 && event_count == 1);
        }
    } else if (!strcmp(test, "missing")) {
        struct uniwill_interface_t *missing[] = {NULL, (void *)(intptr_t)-ENODEV};
        for (unsigned int i = 0; i < sizeof(missing) / sizeof(missing[0]); ++i) {
            reset(); uniwill_interfaces.wmi = missing[i]; int output = 42;
            assert(uniwill_wmi_fn_lock_set(1) == -EIO);
            assert(uniwill_wmi_fn_lock_get(&output) == -EIO && output == 42 && !event_count);
        }
    } else if (!strcmp(test, "sysfs")) {
        const char *invalid[] = {"", "2", "-1", "abc", "1x", "2147483648", "1\n0"};
        for (unsigned int i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i) {
            reset(); assert(uniwill_fn_lock_store(NULL, NULL, invalid[i], strlen(invalid[i])) == -EINVAL && !event_count);
        }
        const char *valid[] = {"0", "0\n", "1", "+1\n"};
        for (unsigned int i = 0; i < sizeof(valid) / sizeof(valid[0]); ++i) {
            reset(); ec[address] = 0xa5;
            size_t size = strlen(valid[i]);
            assert(uniwill_fn_lock_store(NULL, NULL, valid[i], size) == (ssize_t)size);
            char output[8];
            assert(uniwill_fn_lock_show(NULL, NULL, output) == 2);
            assert(!strcmp(output, valid[i][0] == '0' ? "0\n" : "1\n"));
        }
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); char output[8] = "kept"; read_error = errors[i];
            assert(uniwill_fn_lock_show(NULL, NULL, output) == errors[i] && !strcmp(output, "kept"));
            assert(uniwill_fn_lock_store(NULL, NULL, "1", 1) == errors[i] && event_count == 2);
            reset(); write_error = errors[i];
            assert(uniwill_fn_lock_store(NULL, NULL, "1", 1) == errors[i] && event_count == 2);
        }
    } else if (!strcmp(test, "fn_update") || !strcmp(test, "update_fn")) {
        for (unsigned int bit = 1; bit <= 0x80; bit <<= 1) {
            if (bit == UNIWILL_FN_LOCK_MASK) continue;
            for (int on = 0; on <= 1; ++on) {
                reset(); ec[address] = on ? 0 : UNIWILL_FN_LOCK_MASK;
                struct request fn = {.operation=FN, .on=on};
                struct request other = {.operation=UPDATE, .bit=bit};
                bool first = !strcmp(test, "fn_update");
                interleave(first ? fn : other, first ? other : fn);
                assert(ec[address] == (bit | (on ? UNIWILL_FN_LOCK_MASK : 0)) && event_count == 4);
                check_event(0, 1, 'r', address, on ? 0 : UNIWILL_FN_LOCK_MASK);
                assert(events[1].actor == 1 && events[1].kind == 'w');
                assert(events[2].actor == 2 && events[2].kind == 'r');
                assert(events[3].actor == 2 && events[3].kind == 'w');
            }
        }
    } else if (!strcmp(test, "fn_write") || !strcmp(test, "write_fn")) {
        for (int on = 0; on <= 1; ++on) {
            reset(); ec[address] = 0xa5;
            struct request fn = {.operation=FN, .on=on};
            struct request write = {.operation=WRITE, .value=0xc3};
            bool first = !strcmp(test, "fn_write");
            interleave(first ? fn : write, first ? write : fn);
            assert(ec[address] == (first ? 0xc3 : (0xc3 | (on ? UNIWILL_FN_LOCK_MASK : 0))) && event_count == 3);
            if (first) {
                check_event(0, 1, 'r', address, 0xa5); check_event(1, 1, 'w', address, on ? 0xb5 : 0xa5);
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
