/* Verbatim provider wrappers; pthreads and memory replace kernel/EC services. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#define pr_err(...) do {} while (0)
#define pr_debug(...) do {} while (0)
#define IS_ERR_OR_NULL(p) (!(p) || (uintptr_t)(p) >= (uintptr_t)-4095)
#define EXPORT_SYMBOL(name)
/* @INTERFACE_TYPES@ */
static struct { struct uniwill_interface_t *wmi; } uniwill_interfaces;
static pthread_mutex_t harness = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static _Thread_local int actor, lock_depth;
static int lock_calls, unlock_calls;
static bool paused, release_pause, waiting[3], completed[3], gate_read_unlock;
static char pause_kind;
struct event { int actor; char kind; u16 address; u8 value; };
static struct event events[16];
static int event_count, read_error, write_error;
static int read_sequence[8], read_sequence_length, read_sequence_position;
static bool failed_write_applies;
static u8 ec[UINT16_MAX + 1u];
struct mutex { pthread_mutex_t native; };
#define DEFINE_MUTEX(name) struct mutex name = {PTHREAD_MUTEX_INITIALIZER}
static void mutex_lock(struct mutex *lock)
{
    assert(lock_depth == 0); /* Re-entering a nonrecursive kernel mutex deadlocks. */
    int status = pthread_mutex_trylock(&lock->native);
    if (status == EBUSY) {
        pthread_mutex_lock(&harness);
        waiting[actor] = true;
        pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&harness);
        status = pthread_mutex_lock(&lock->native);
    }
    assert(status == 0);
    lock_depth = 1;
    pthread_mutex_lock(&harness); ++lock_calls; pthread_mutex_unlock(&harness);
}
static void mutex_unlock(struct mutex *lock)
{
    assert(lock_depth == 1);
    pthread_mutex_lock(&harness);
    bool yield_after_read = gate_read_unlock && actor == 1 && event_count == 1 && events[0].kind == 'r';
    ++unlock_calls;
    pthread_mutex_unlock(&harness);
    lock_depth = 0;
    assert(pthread_mutex_unlock(&lock->native) == 0);
    if (yield_after_read) {
        /* A legal scheduling point when a broken update releases after read. */
        pthread_mutex_lock(&harness);
        struct timespec end; assert(clock_gettime(CLOCK_REALTIME, &end) == 0); end.tv_sec += 3;
        while (!completed[2]) assert(pthread_cond_timedwait(&changed, &harness, &end) == 0);
        pthread_mutex_unlock(&harness);
    }
}
static void record(char kind, u16 address, u8 value)
{
    assert(event_count < (int)(sizeof(events) / sizeof(events[0])));
    events[event_count++] = (struct event){actor, kind, address, value};
    if (actor == 1 && pause_kind == kind && !paused) {
        paused = true; pthread_cond_broadcast(&changed);
        while (!release_pause) pthread_cond_wait(&changed, &harness);
    }
}
static int provider_read(u16 address, u8 *data)
{
    assert(lock_depth == 1);
    pthread_mutex_lock(&harness);
    int status = IS_ERR_OR_NULL(data) ? -EINVAL :
        (read_sequence_position < read_sequence_length ? read_sequence[read_sequence_position++] : read_error);
    if (!status) *data = ec[address];
    record('r', address, ec[address]);
    pthread_mutex_unlock(&harness);
    return status;
}
static int provider_write(u16 address, u8 data)
{
    assert(lock_depth == 1);
    pthread_mutex_lock(&harness);
    int status = write_error;
    if (!status || failed_write_applies) ec[address] = data;
    record('w', address, data);
    pthread_mutex_unlock(&harness);
    return status;
}
static struct uniwill_interface_t transport = {.read_ec_ram = provider_read, .write_ec_ram = provider_write};
/* @DRIVER_FUNCTIONS@ */
static void reset(void)
{
    assert(lock_depth == 0 && lock_calls == unlock_calls);
    uniwill_interfaces.wmi = &transport;
    event_count = read_error = write_error = 0;
    read_sequence_length = read_sequence_position = 0;
    failed_write_applies = paused = release_pause = gate_read_unlock = false;
    memset(waiting, 0, sizeof(waiting)); memset(completed, 0, sizeof(completed));
    actor = 0; pause_kind = 0;
}
static void check_event(int index, int who, char kind, u16 address, u8 value)
{
    assert(index < event_count);
    assert(events[index].actor == who && events[index].kind == kind);
    assert(events[index].address == address && events[index].value == value);
}
enum operation { READ, WRITE, UPDATE };
struct request { enum operation operation; u8 clear, set, value, output; int status; };
static struct request requests[3];
static void *run(void *argument)
{
    actor = (int)(intptr_t)argument;
    struct request *r = &requests[actor];
    if (r->operation == READ) r->status = uniwill_read_ec_ram(0x078c, &r->output);
    else if (r->operation == WRITE) r->status = uniwill_write_ec_ram(0x078c, r->value);
    else r->status = uniwill_update_ec_ram(0x078c, r->clear, r->set);
    assert(lock_depth == 0);
    pthread_mutex_lock(&harness); completed[actor] = true;
    pthread_cond_broadcast(&changed); pthread_mutex_unlock(&harness);
    return NULL;
}
static void interleave(struct request first, struct request second)
{
    requests[1] = first; requests[2] = second;
    pause_kind = first.operation == WRITE ? 'w' : 'r'; gate_read_unlock = true;
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
    assert(requests[1].status == 0 && requests[2].status == 0);
    assert(lock_calls == unlock_calls);
}
static const int errors[] = {-EIO, -ENODEV, -ETIMEDOUT, -EINVAL, -ENOMEM, 7};
int main(int argc, char **argv)
{
    assert(argc == 2); const char *test = argv[1]; reset();
    if (!strcmp(test, "all_bytes_masks")) {
        for (unsigned int original = 0; original <= UINT8_MAX; ++original)
            for (unsigned int clear = 0; clear <= UINT8_MAX; ++clear)
                for (unsigned int set = 0; set <= UINT8_MAX; ++set) {
                    ec[0x078c] = original; event_count = 0;
                    assert(uniwill_update_ec_ram(0x078c, clear, set) == 0);
                    for (unsigned int bit = 1; bit <= 0x80; bit <<= 1) {
                        if (set & bit) assert(ec[0x078c] & bit);
                        else if (clear & bit) assert(!(ec[0x078c] & bit));
                        else assert((ec[0x078c] & bit) == (original & bit));
                    }
                    assert(event_count == 2);
                    check_event(0, 0, 'r', 0x078c, original);
                    check_event(1, 0, 'w', 0x078c, ec[0x078c]);
                }
    } else if (!strcmp(test, "addresses")) {
        static const u16 addresses[] = {0, 1, 0x078c, 0x0767, 0xffff};
        for (unsigned int i = 0; i < sizeof(addresses) / sizeof(addresses[0]); ++i) {
            reset(); ec[addresses[i]] = 0xa5;
            assert(uniwill_update_ec_ram(addresses[i], 0x0f, 0x03) == 0);
            check_event(0, 0, 'r', addresses[i], 0xa5); check_event(1, 0, 'w', addresses[i], 0xa3);
        }
    } else if (!strcmp(test, "unchanged")) {
        for (unsigned int value = 0; value <= UINT8_MAX; ++value) {
            reset(); ec[0x078c] = value;
            assert(uniwill_update_ec_ram(0x078c, 0, 0) == 0);
            assert(event_count == 2 && ec[0x078c] == value);
        }
    } else if (!strcmp(test, "read_errors") || !strcmp(test, "write_errors") || !strcmp(test, "partial_write_error")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); ec[0x078c] = 0x81;
            bool reading = !strcmp(test, "read_errors");
            if (reading) read_error = errors[i]; else write_error = errors[i];
            failed_write_applies = !strcmp(test, "partial_write_error");
            assert(uniwill_update_ec_ram(0x078c, 0x0f, 0x05) == errors[i]);
            assert(event_count == (reading ? 1 : 2));
            assert(ec[0x078c] == (failed_write_applies ? 0x85 : 0x81));
            read_error = write_error = 0; event_count = 0;
            assert(uniwill_update_ec_ram(0x078c, 0x0f, 0x07) == 0 && ec[0x078c] == 0x87);
        }
    } else if (!strcmp(test, "missing_interface")) {
        struct uniwill_interface_t *missing[] = {NULL, (void *)(intptr_t)-ENODEV};
        for (unsigned int i = 0; i < sizeof(missing) / sizeof(missing[0]); ++i) {
            reset(); uniwill_interfaces.wmi = missing[i]; u8 value = 0xa5;
            assert(uniwill_read_ec_ram(0xffff, &value) == -EIO && value == 0xa5);
            assert(uniwill_read_ec_ram(0xffff, NULL) == -EIO);
            assert(uniwill_write_ec_ram(0xffff, 0x53) == -EIO);
            assert(uniwill_update_ec_ram(0xffff, 0xff, 0x53) == -EIO && !event_count);
        }
    } else if (!strcmp(test, "null_read")) {
        assert(uniwill_read_ec_ram(0x1234, NULL) == -EINVAL);
        assert(uniwill_read_ec_ram(0x4321, (void *)(intptr_t)-EINVAL) == -EINVAL);
        assert(event_count == 2 && events[0].address == 0x1234 && events[1].address == 0x4321);
    } else if (!strcmp(test, "plain_read")) {
        for (unsigned int value = 0; value <= UINT8_MAX; ++value) {
            reset(); ec[0x1234] = value; u8 output = value ^ 0xff;
            assert(uniwill_read_ec_ram(0x1234, &output) == 0 && output == value);
            check_event(0, 0, 'r', 0x1234, value);
        }
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); read_error = errors[i]; u8 output = 0xa5;
            assert(uniwill_read_ec_ram(0x1234, &output) == errors[i] && output == 0xa5 && event_count == 1);
        }
    } else if (!strcmp(test, "plain_write")) {
        for (unsigned int value = 0; value <= UINT8_MAX; ++value) {
            reset(); assert(uniwill_write_ec_ram(0x4321, value) == 0 && ec[0x4321] == value);
            check_event(0, 0, 'w', 0x4321, value);
        }
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
            reset(); ec[0x4321] = 0xa5; write_error = errors[i];
            assert(uniwill_write_ec_ram(0x4321, 0x53) == errors[i] && ec[0x4321] == 0xa5 && event_count == 1);
        }
    } else if (!strcmp(test, "update_write")) {
        ec[0x078c] = 0xf0;
        interleave((struct request){.operation=UPDATE,.clear=0x0f,.set=5}, (struct request){.operation=WRITE,.value=0xc3});
        assert(ec[0x078c] == 0xc3 && event_count == 3);
        check_event(0, 1, 'r', 0x078c, 0xf0); check_event(1, 1, 'w', 0x078c, 0xf5); check_event(2, 2, 'w', 0x078c, 0xc3);
    } else if (!strcmp(test, "write_update")) {
        ec[0x078c] = 0xf0;
        interleave((struct request){.operation=WRITE,.value=0xc3}, (struct request){.operation=UPDATE,.clear=0x0f,.set=5});
        assert(ec[0x078c] == 0xc5 && event_count == 3);
        check_event(0, 1, 'w', 0x078c, 0xc3); check_event(1, 2, 'r', 0x078c, 0xc3); check_event(2, 2, 'w', 0x078c, 0xc5);
    } else if (!strcmp(test, "two_updates") || !strcmp(test, "overlap_updates")) {
        bool overlap = !strcmp(test, "overlap_updates");
        struct request low = {.operation=UPDATE,.clear=0x0f,.set=5};
        struct request high = {.operation=UPDATE,.clear=overlap ? 0x0f : 0xf0,.set=overlap ? 3 : 0xa0};
        for (int reverse = 0; reverse < 2; ++reverse) {
            reset(); ec[0x078c] = 0x12; interleave(reverse ? high : low, reverse ? low : high);
            assert(ec[0x078c] == (overlap ? (reverse ? 0x15 : 0x13) : 0xa5));
            assert(event_count == 4 && events[0].actor == 1 && events[1].actor == 1 && events[2].actor == 2 && events[3].actor == 2);
            assert(events[0].kind == 'r' && events[1].kind == 'w' && events[2].kind == 'r' && events[3].kind == 'w');
        }
    } else if (!strcmp(test, "update_read")) {
        ec[0x078c] = 0xf0;
        interleave((struct request){.operation=UPDATE,.clear=0x0f,.set=5}, (struct request){.operation=READ});
        assert(requests[2].output == 0xf5 && event_count == 3);
        check_event(2, 2, 'r', 0x078c, 0xf5);
    } else if (!strcmp(test, "read_update")) {
        ec[0x078c] = 0xf0;
        interleave((struct request){.operation=READ}, (struct request){.operation=UPDATE,.clear=0x0f,.set=5});
        assert(requests[1].output == 0xf0 && ec[0x078c] == 0xf5 && event_count == 3);
        check_event(0, 1, 'r', 0x078c, 0xf0); check_event(1, 2, 'r', 0x078c, 0xf0); check_event(2, 2, 'w', 0x078c, 0xf5);
    } else if (!strcmp(test, "lock_release")) {
        for (int cycle = 0; cycle < 1000; ++cycle) {
            reset(); read_error = -EIO;
            assert(uniwill_update_ec_ram(0x078c, 0xff, 3) == -EIO);
            read_error = 0; write_error = -ETIMEDOUT;
            assert(uniwill_update_ec_ram(0x078c, 0xff, 3) == -ETIMEDOUT);
            write_error = 0; assert(uniwill_write_ec_ram(0x078c, 9) == 0);
            u8 value; assert(uniwill_read_ec_ram(0x078c, &value) == 0 && value == 9);
        }
    } else if (!strcmp(test, "retry_invalid")) {
        const int budgets[] = {0, -1, INT_MIN};
        for (unsigned int i = 0; i < sizeof(budgets) / sizeof(budgets[0]); ++i) {
            reset(); u8 value = 0xa5; int calls = lock_calls;
            assert(uniwill_read_ec_ram_with_retry(0x1234, &value, budgets[i]) == -EINVAL);
            assert(uniwill_read_ec_ram_with_retry(0x1234, NULL, budgets[i]) == -EINVAL);
            assert(value == 0xa5 && event_count == 0 && lock_calls == calls);
        }
    } else if (!strcmp(test, "retry_first_success")) {
        const int budgets[] = {1, 3, INT_MAX};
        for (unsigned int i = 0; i < sizeof(budgets) / sizeof(budgets[0]); ++i) {
            reset(); ec[0xffff] = 0xd3; u8 value = 0;
            assert(uniwill_read_ec_ram_with_retry(0xffff, &value, budgets[i]) == 0);
            assert(value == 0xd3 && event_count == 1); check_event(0, 0, 'r', 0xffff, 0xd3);
        }
    } else if (!strcmp(test, "retry_transient")) {
        reset(); ec[0x1234] = 0xc7; u8 value = 0xa5;
        read_sequence[0] = -EIO; read_sequence[1] = -ETIMEDOUT; read_sequence[2] = 0; read_sequence_length = 3;
        assert(uniwill_read_ec_ram_with_retry(0x1234, &value, 5) == 0);
        assert(value == 0xc7 && event_count == 3 && read_sequence_position == 3);
        for (int i = 0; i < event_count; ++i) check_event(i, 0, 'r', 0x1234, 0xc7);
    } else if (!strcmp(test, "retry_exhausted")) {
        for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i)
            for (int attempts = 1; attempts <= 5; ++attempts) {
                reset(); read_error = errors[i]; u8 value = 0xa5;
                assert(uniwill_read_ec_ram_with_retry(0x1234, &value, attempts) == errors[i]);
                assert(value == 0xa5 && event_count == attempts);
                for (int n = 0; n < attempts; ++n) assert(events[n].kind == 'r');
            }
    } else if (!strcmp(test, "retry_budget")) {
        reset(); ec[0x1234] = 0xc7; u8 value = 0xa5;
        read_sequence[0] = -EIO; read_sequence[1] = -ETIMEDOUT; read_sequence[2] = 0; read_sequence_length = 3;
        assert(uniwill_read_ec_ram_with_retry(0x1234, &value, 2) == -ETIMEDOUT);
        assert(value == 0xa5 && event_count == 2 && read_sequence_position == 2);
    } else if (!strcmp(test, "retry_missing")) {
        uniwill_interfaces.wmi = NULL; u8 value = 0xa5; int calls = lock_calls;
        assert(uniwill_read_ec_ram_with_retry(0x1234, &value, 3) == -EIO);
        assert(value == 0xa5 && event_count == 0 && lock_calls == calls + 3);
    } else if (!strcmp(test, "retry_null")) {
        assert(uniwill_read_ec_ram_with_retry(0x1234, NULL, 3) == -EINVAL);
        assert(event_count == 3);
        for (int i = 0; i < event_count; ++i) assert(events[i].kind == 'r' && events[i].address == 0x1234);
    } else return 2;
    assert(lock_depth == 0 && lock_calls == unlock_calls);
    return 0;
}
