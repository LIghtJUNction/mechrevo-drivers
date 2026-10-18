/* Actual file lifecycle and new ioctl handler; provider is a boundary spy.
 * Provider EC transactions/leases are covered by the separate real-provider suite. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
typedef uint64_t u64;
#define __user
#define static_assert _Static_assert
#define GFP_KERNEL 0
#define CAP_SYS_ADMIN 21
#define FMODE_WRITE 2
/* Linux reserves this token as a task accessor macro. Keep the real
 * expansion shape so userspace fixtures catch kernel-only name collisions. */
#define current get_current()
/* @UAPI_HEADER@ */
struct inode { int unused; };
struct file { void *private_data; unsigned f_mode; };
static bool allocation_fail, administrator = true;
static unsigned allocations, frees, from_calls, to_calls, provider_calls, close_calls;
static _Thread_local int actor;
static _Thread_local bool from_fail, to_fail;
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static bool paused, unpause, waiting[3], completed[3], pause_acquire, failing_first_ack;
static int thread_status[3];
struct mutex { pthread_mutex_t native; };
static void mutex_init(struct mutex *mutex) { assert(!pthread_mutex_init(&mutex->native, NULL)); }
static void mutex_lock(struct mutex *mutex) {
    int status = pthread_mutex_trylock(&mutex->native);
    if (status == EBUSY) {
        pthread_mutex_lock(&gate); waiting[actor] = true;
        pthread_cond_broadcast(&changed); pthread_mutex_unlock(&gate);
        status = pthread_mutex_lock(&mutex->native);
    }
    assert(status == 0);
}
static void mutex_unlock(struct mutex *mutex) { assert(!pthread_mutex_unlock(&mutex->native)); }
static void *kzalloc(size_t size, int flags) { (void)flags; if (allocation_fail) return NULL; ++allocations; return calloc(1, size); }
static void kfree(void *pointer) { if (pointer) ++frees; free(pointer); }
static bool capable(int capability) { assert(capability == CAP_SYS_ADMIN); return administrator; }
static size_t copy_from_user(void *out, const void *in, size_t size) {
    ++from_calls; if (from_fail || !in) return size; memcpy(out, in, size); return 0;
}
static size_t copy_to_user(void *out, const void *in, size_t size) {
    ++to_calls; if (to_fail || !out) return size; memcpy(out, in, size); return 0;
}
typedef _Atomic uint64_t atomic64_t;
#define ATOMIC64_INIT(value) (value)
static uint64_t atomic64_inc_return(atomic64_t *value) { return atomic_fetch_add(value, 1) + 1; }
static u64 provider_owner, provider_epoch = 7, provider_generation = 9, provider_lease, lease_sequence;
static int provider_error;
static struct uw_fan_curve_control_v1 header(size_t size) {
    return (struct uw_fan_curve_control_v1){.version = UW_FAN_CURVE_ABI_VERSION, .size = size,
        .epoch = provider_epoch, .generation = provider_generation, .lease_id = provider_lease};
}
static int uw_fan_curve_get_state(u64 owner, struct uw_fan_curve_state_v1 *state) {
    ++provider_calls; if (provider_error) return provider_error;
    memset(state, 0, sizeof(*state)); state->control = header(sizeof(*state));
    if (owner == provider_owner && provider_lease) state->state_flags = UW_FAN_CURVE_STATE_OWNER;
    return 0;
}
static int uw_fan_curve_acquire(u64 owner, struct uw_fan_curve_control_v1 *control) {
    ++provider_calls; if (provider_error) return provider_error;
    pthread_mutex_lock(&gate);
    if (provider_lease) { pthread_mutex_unlock(&gate); return -EBUSY; }
    provider_owner = owner; provider_lease = ++lease_sequence; ++provider_generation;
    *control = header(sizeof(*control));
    if (pause_acquire && actor == 1) {
        paused = true; pthread_cond_broadcast(&changed);
        while (!unpause) pthread_cond_wait(&changed, &gate);
    }
    pthread_mutex_unlock(&gate); return 0;
}
static int uw_fan_curve_apply(u64 owner, const struct uw_fan_curve_apply_v1 *request) {
    ++provider_calls; (void)request; assert(owner); return provider_error;
}
static int uw_fan_curve_release(u64 owner, const struct uw_fan_curve_control_v1 *control) {
    ++provider_calls; if (provider_error) return provider_error;
    pthread_mutex_lock(&gate);
    int status = owner != provider_owner || control->lease_id != provider_lease ? -ESTALE : 0;
    if (!status) { provider_owner = provider_lease = 0; ++provider_generation; }
    pthread_mutex_unlock(&gate); return status;
}
static void uw_fan_curve_close(u64 owner, u64 epoch, u64 lease) {
    ++close_calls;
    pthread_mutex_lock(&gate);
    if (owner == provider_owner && epoch == provider_epoch && lease == provider_lease)
        provider_owner = provider_lease = 0;
    pthread_mutex_unlock(&gate);
}
/* @ACTUAL_FILE_LIFECYCLE_AND_IOCTL@ */
static long call(struct file *file, unsigned int command, void *data) {
    return uw_fan_curve_ioctl(file, command, (unsigned long)data);
}
static void open_file(struct file *file) { file->f_mode = FMODE_WRITE; assert(fop_open(NULL, file) == 0 && file->private_data); }
static void close_file(struct file *file) { assert(fop_release(NULL, file) == 0 && !file->private_data); }
static void acquire(struct file *file, struct uw_fan_curve_control_v1 *control) {
    *control = header(sizeof(*control)); control->lease_id = 0;
    assert(call(file, UW_FAN_CURVE_ACQUIRE, control) == 0 && control->lease_id);
}
static void unique_files(void) {
    struct file one = {0}, two = {0};
    allocation_fail = true; assert(fop_open(NULL, &one) == -ENOMEM && !one.private_data);
    allocation_fail = false; open_file(&one); open_file(&two);
    struct uw_curve_file_context *a = one.private_data, *b = two.private_data;
    assert(a->owner && b->owner && a->owner != b->owner);
    struct uw_fan_curve_control_v1 control; acquire(&one, &control);
    close_file(&two); assert(close_calls == 0 && provider_lease == control.lease_id);
    close_file(&one); assert(close_calls == 1 && !provider_lease && allocations == frees);
}
static void permission_and_shape(void) {
    struct file file = {0}; open_file(&file);
    struct uw_fan_curve_state_v1 state = {.control = header(sizeof(state))};
    administrator = false;
    assert(call(&file, UW_FAN_CURVE_GET_STATE, &state) == 0);
    unsigned before = provider_calls, copies = from_calls;
    unsigned commands[] = {UW_FAN_CURVE_ACQUIRE, UW_FAN_CURVE_APPLY, UW_FAN_CURVE_RESTORE_RELEASE};
    for (unsigned i = 0; i < 3; ++i) assert(call(&file, commands[i], &state) == -EPERM);
    assert(provider_calls == before && from_calls == copies);
    administrator = true;
    file.f_mode = 0;
    for (unsigned i = 0; i < 3; ++i) assert(call(&file, commands[i], &state) == -EBADF);
    assert(call(&file, UW_FAN_CURVE_GET_STATE, &state) == 0);
    before = provider_calls; copies = from_calls;
    file.f_mode = FMODE_WRITE;
    unsigned invalid[] = {_IO(UW_FAN_CURVE_MAGIC, 0), _IOWR(UW_FAN_CURVE_MAGIC, 0, struct uw_fan_curve_control_v1),
        _IOWR(UW_FAN_CURVE_MAGIC, 9, struct uw_fan_curve_state_v1), UW_FAN_CURVE_APPLY ^ (1u << _IOC_DIRSHIFT)};
    for (unsigned i = 0; i < sizeof(invalid)/sizeof(invalid[0]); ++i)
        assert(call(&file, invalid[i], &state) == -ENOTTY);
    assert(provider_calls == before && from_calls == copies);
    assert(call(NULL, UW_FAN_CURVE_GET_STATE, &state) == -EBADF);
    struct file absent = {0}; assert(call(&absent, UW_FAN_CURVE_GET_STATE, &state) == -EBADF);
    close_file(&file);
}
static void failed_copies_and_receipts(void) {
    struct file file = {0}; open_file(&file);
    struct uw_fan_curve_state_v1 state = {.control = header(sizeof(state))};
    unsigned commands[] = {UW_FAN_CURVE_GET_STATE, UW_FAN_CURVE_ACQUIRE, UW_FAN_CURVE_APPLY, UW_FAN_CURVE_RESTORE_RELEASE};
    from_fail = true;
    for (unsigned i = 0; i < 4; ++i) assert(call(&file, commands[i], &state) == -EFAULT);
    assert(!provider_calls); from_fail = false;
    struct uw_fan_curve_control_v1 control = header(sizeof(control)); control.lease_id = 0;
    to_fail = true; assert(call(&file, UW_FAN_CURVE_ACQUIRE, &control) == -EFAULT);
    assert(close_calls == 1 && !provider_lease);
    struct uw_curve_file_context *context = file.private_data;
    assert(!context->epoch && !context->lease_id); to_fail = false;
    acquire(&file, &control);
    provider_error = -EREMOTEIO;
    assert(call(&file, UW_FAN_CURVE_RESTORE_RELEASE, &control) == -EREMOTEIO);
    assert(context->lease_id == control.lease_id && provider_lease == control.lease_id);
    struct uw_fan_curve_apply_v1 request = {.control = control};
    assert(call(&file, UW_FAN_CURVE_APPLY, &request) == -EREMOTEIO);
    provider_error = 0;
    assert(call(&file, UW_FAN_CURVE_RESTORE_RELEASE, &control) == 0);
    assert(!context->lease_id && !context->epoch && !provider_lease);
    close_file(&file); assert(close_calls == 1);
}
static void revoked_receipt(void) {
    struct file file = {0}; open_file(&file);
    struct uw_fan_curve_control_v1 control; acquire(&file, &control);
    struct uw_fan_curve_state_v1 state = {.control = header(sizeof(state))};
    assert(call(&file, UW_FAN_CURVE_GET_STATE, &state) == 0);
    assert((state.state_flags & UW_FAN_CURVE_STATE_OWNER) && !(state.state_flags & UW_FAN_CURVE_STATE_REVOKED));
    ++provider_epoch;
    assert(call(&file, UW_FAN_CURVE_GET_STATE, &state) == 0);
    assert(state.state_flags & UW_FAN_CURVE_STATE_REVOKED);
    provider_owner = provider_lease = 0; close_file(&file);
}
static struct file shared_file;
static void *first_acquire(void *unused) {
    (void)unused; actor = 1; to_fail = failing_first_ack;
    struct uw_fan_curve_control_v1 control = header(sizeof(control)); control.lease_id = 0;
    thread_status[1] = (int)call(&shared_file, UW_FAN_CURVE_ACQUIRE, &control);
    pthread_mutex_lock(&gate); completed[1] = true; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&gate);
    return NULL;
}
static void *release_and_reacquire(void *unused) {
    (void)unused; actor = 2;
    pthread_mutex_lock(&gate); struct uw_fan_curve_control_v1 control = header(sizeof(control)); pthread_mutex_unlock(&gate);
    int status;
    if (failing_first_ack) {
        control.lease_id = 0;
        status = (int)call(&shared_file, UW_FAN_CURVE_ACQUIRE, &control);
    } else {
        status = (int)call(&shared_file, UW_FAN_CURVE_RESTORE_RELEASE, &control);
    }
    if (!status && !failing_first_ack) {
        control = header(sizeof(control)); control.lease_id = 0;
        status = (int)call(&shared_file, UW_FAN_CURVE_ACQUIRE, &control);
    }
    thread_status[2] = status;
    pthread_mutex_lock(&gate); completed[2] = true; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&gate);
    return NULL;
}
static void wait_for(bool *value, bool *alternative) {
    struct timespec until; assert(!clock_gettime(CLOCK_REALTIME, &until)); until.tv_sec += 3;
    pthread_mutex_lock(&gate);
    while (!*value && (!alternative || !*alternative))
        assert(!pthread_cond_timedwait(&changed, &gate, &until));
    pthread_mutex_unlock(&gate);
}
static void same_file_serializes(bool ack_fails) {
    failing_first_ack = ack_fails;
    open_file(&shared_file); pause_acquire = true;
    pthread_t one, two;
    assert(!pthread_create(&one, NULL, first_acquire, NULL)); wait_for(&paused, NULL);
    assert(!pthread_create(&two, NULL, release_and_reacquire, NULL)); wait_for(&waiting[2], &completed[2]);
    pthread_mutex_lock(&gate); unpause = true; pthread_cond_broadcast(&changed); pthread_mutex_unlock(&gate);
    assert(!pthread_join(one, NULL) && !pthread_join(two, NULL));
    assert(thread_status[1] == (ack_fails ? -EFAULT : 0) && thread_status[2] == 0);
    struct uw_curve_file_context *context = shared_file.private_data;
    assert(context->lease_id == provider_lease && provider_lease == 2);
    close_file(&shared_file); assert(!provider_lease && close_calls == (ack_fails ? 2u : 1u));
}
int main(int argc, char **argv) {
    assert(argc == 2);
    if (!strcmp(argv[1], "unique_files")) unique_files();
    else if (!strcmp(argv[1], "permission_and_shape")) permission_and_shape();
    else if (!strcmp(argv[1], "failed_copies_and_receipts")) failed_copies_and_receipts();
    else if (!strcmp(argv[1], "revoked_receipt")) revoked_receipt();
    else if (!strcmp(argv[1], "same_file_serializes")) same_file_serializes(false);
    else if (!strcmp(argv[1], "same_file_failed_ack")) same_file_serializes(true);
    else assert(false);
    printf("{\"case\":\"%s\",\"provider_calls\":%u,\"close_calls\":%u}\n", argv[1], provider_calls, close_calls);
    return 0;
}
