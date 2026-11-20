
struct work_struct { void (*function)(struct work_struct *); };
struct delayed_work { struct work_struct work; bool pending; unsigned long delay; };
#define DECLARE_DELAYED_WORK(name, fn) struct delayed_work name = { .work.function = (fn) }
static void *system_wq;
static pthread_mutex_t state_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t state_changed = PTHREAD_COND_INITIALIZER;
static int queued, cancelled, firmware_calls;
static bool worker_running, stop_started, stop_done, block_read, entered, allow_return;
static bool reentrant_notify;
static u64 firmware_profile, firmware_write_result;
static int firmware_error;
static struct wmi_device device;

void rewrite_last_profile(void);
static void cancel_delayed_work_sync(struct delayed_work *work);
static void mod_delayed_work(void *queue, struct delayed_work *work, unsigned long delay)
{
    pthread_mutex_lock(&state_lock);
    ++queued;
    work->pending = true;
    work->delay = delay;
    pthread_mutex_unlock(&state_lock);
}
static acpi_status wmidev_evaluate_method(struct wmi_device *wdev, int instance,
                                          u32 method, struct acpi_buffer *in,
                                          struct acpi_buffer *out)
{
    assert(wdev == &device && in->length == sizeof(u64));
    ++firmware_calls;
    if (method == 2) {
        assert(*(u64 *)in->pointer == 0); /* No uninitialized firmware arguments. */
        if (reentrant_notify)
            rewrite_last_profile();
        pthread_mutex_lock(&state_lock);
        if (block_read) {
            entered = true;
            pthread_cond_broadcast(&state_changed);
            while (!allow_return)
                pthread_cond_wait(&state_changed, &state_lock);
        }
        pthread_mutex_unlock(&state_lock);
    }
    union acpi_object *object = calloc(1, sizeof(*object));
    assert(object);
    object->type = ACPI_TYPE_INTEGER;
    object->integer.value = method == 2 ? firmware_profile : firmware_write_result;
    out->pointer = object;
    if (!firmware_error && method == 1 && !firmware_write_result)
        firmware_profile = *(u64 *)in->pointer;
    return firmware_error;
}

@FUNCTIONS@

static void cancel_delayed_work_sync(struct delayed_work *work)
{
    /* Teardown may wait for a worker; it must not hold the worker's mutex. */
    assert(!pthread_mutex_trylock(&nb05_wmi_aa_access_lock.native));
    pthread_mutex_unlock(&nb05_wmi_aa_access_lock.native);
    assert(__wmi_dev == &device); /* Another probe cannot publish during cancel. */
    int before = queued;
    rewrite_last_profile(); /* A notification racing cancellation cannot re-arm. */
    assert(queued == before);
    pthread_mutex_lock(&state_lock);
    while (worker_running)
        pthread_cond_wait(&state_changed, &state_lock);
    work->pending = false;
    ++cancelled;
    pthread_mutex_unlock(&state_lock);
}
static void *worker_thread(void *unused)
{
    nb05_rewrite_profile_work_handler(&nb05_rewrite_profile_work.work);
    pthread_mutex_lock(&state_lock);
    worker_running = false;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    return NULL;
}
static void *stop_thread(void *unused)
{
    pthread_mutex_lock(&state_lock);
    stop_started = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    stop_profile_work(&device);
    pthread_mutex_lock(&state_lock);
    stop_done = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    return NULL;
}
int main(void)
{
    char buffer[4096];
    struct driver_data_t data = { .last_chosen_profile = 0 };
    struct device sysfs_dev = { .drvdata = &data };
    device.dev.drvdata = &data;
    __wmi_dev = &device;
    profile_work_enabled = true;

    memset(buffer, 'x', sizeof(buffer));
    assert(platform_profile_choices_show(&sysfs_dev, NULL, buffer) ==
           strlen("low-power balanced performance\n"));
    assert(!strcmp(buffer, "low-power balanced performance\n"));
    firmware_error = 1;
    assert(platform_profile_store(&sysfs_dev, NULL, "performance\n", 12) == -EIO);
    assert(data.last_chosen_profile == 0 && !profile_changed_by_driver());
    assert(queued == 0 && freed_objects == 1);
    firmware_error = 0;
    firmware_write_result = 5;
    assert(platform_profile_store(&sysfs_dev, NULL, "performance", 11) == -EINVAL);
    assert(data.last_chosen_profile == 0 && queued == 0);
    firmware_write_result = 0;
    assert(platform_profile_store(&sysfs_dev, NULL, "performance\n", 12) == 12);
    assert(data.last_chosen_profile == 1 && firmware_profile == 1);
    assert(profile_changed_by_driver() && queued == 1);
    assert(nb05_rewrite_profile_work.delay == 300);
    int before = firmware_calls;
    assert(platform_profile_store(&sysfs_dev, NULL, "bogus", 5) == -EINVAL);
    assert(firmware_calls == before);

    firmware_profile = 2;
    nb05_rewrite_profile_work_handler(&nb05_rewrite_profile_work.work);
    assert(firmware_profile == 1 && data.last_chosen_profile == 1 && queued == 2);
    nb05_rewrite_profile_work_handler(&nb05_rewrite_profile_work.work);
    assert(!profile_changed_by_driver());
    reentrant_notify = true;
    assert(platform_profile_show(&sysfs_dev, NULL, buffer) == 12);
    assert(!strcmp(buffer, "performance\n"));
    reentrant_notify = false;
    firmware_error = 1;
    assert(platform_profile_show(&sysfs_dev, NULL, buffer) == -EIO);
    firmware_error = 0;

    pthread_t worker, stopper;
    block_read = true;
    worker_running = true;
    assert(!pthread_create(&worker, NULL, worker_thread, NULL));
    pthread_mutex_lock(&state_lock);
    while (!entered)
        pthread_cond_wait(&state_changed, &state_lock);
    pthread_mutex_unlock(&state_lock);
    assert(!pthread_create(&stopper, NULL, stop_thread, NULL));
    pthread_mutex_lock(&state_lock);
    while (!stop_started)
        pthread_cond_wait(&state_changed, &state_lock);
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += 50000000;
    if (deadline.tv_nsec >= 1000000000) {
        ++deadline.tv_sec;
        deadline.tv_nsec -= 1000000000;
    }
    while (!stop_done) {
        int result = pthread_cond_timedwait(&state_changed, &state_lock, &deadline);
        if (result == ETIMEDOUT)
            break;
        assert(!result);
    }
    assert(!stop_done);
    allow_return = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    pthread_join(worker, NULL);
    pthread_join(stopper, NULL);

    assert(cancelled == 1 && stop_done && data.stopping && !__wmi_dev);
    assert(!nb05_rewrite_profile_work.pending && !profile_changed_by_driver());
    before = firmware_calls;
    int queued_before = queued;
    rewrite_last_profile();
    nb05_rewrite_profile_work_handler(&nb05_rewrite_profile_work.work);
    assert(platform_profile_store(&sysfs_dev, NULL, "balanced", 8) == -ENODEV);
    assert(data.last_chosen_profile == 1 && firmware_calls == before);
    assert(queued == queued_before);
    return 0;
}
