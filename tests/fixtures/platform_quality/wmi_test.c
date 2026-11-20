
static struct wmi_device device;
static u8 input[496], output[80];
static unsigned expected_length = 80;
static size_t last_input_length;
static bool integer_reply, block_call, entered, allow_return;
static bool remove_started, remove_done;
static int fault, firmware_calls, call_result;
static pthread_mutex_t state_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t state_changed = PTHREAD_COND_INITIALIZER;

static acpi_status wmidev_evaluate_method(struct wmi_device *wdev, int instance,
                                          u32 method, struct acpi_buffer *in,
                                          struct acpi_buffer *out)
{
    assert(wdev == &device);
    ++firmware_calls;
    last_input_length = in->length;
    pthread_mutex_lock(&state_lock);
    if (block_call) {
        entered = true;
        pthread_cond_broadcast(&state_changed);
        while (!allow_return)
            pthread_cond_wait(&state_changed, &state_lock);
    }
    pthread_mutex_unlock(&state_lock);

    if (fault == 1)
        return 0; /* Missing result object. */
    union acpi_object *object = calloc(1, sizeof(*object) + 80);
    assert(object);
    out->pointer = object;
    if (integer_reply) {
        object->type = ACPI_TYPE_INTEGER;
        object->integer.value = 42;
    } else {
        object->type = ACPI_TYPE_BUFFER;
        object->buffer.length = fault == 3 ? 7 : expected_length;
        object->buffer.pointer = fault == 4 ? NULL : (u8 *)(object + 1);
        if (object->buffer.pointer)
            memset(object->buffer.pointer, 0x3a, 80);
    }
    return fault == 5 ? 1 : 0; /* Failure with an allocated output. */
}

@FUNCTIONS@

static void *call_thread(void *unused)
{
    call_result = CALL(2, input, output);
    return NULL;
}
static void *remove_thread(void *unused)
{
    pthread_mutex_lock(&state_lock);
    remove_started = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    REMOVE(&device);
    pthread_mutex_lock(&state_lock);
    remove_done = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    return NULL;
}

int main(void)
{
    memset(output, 0x77, sizeof(output));
    assert(CALL(2, input, output) == -ENODEV);
    assert(firmware_calls == 0 && output[0] == 0x77);
    __wmi_dev = &device;
    assert(CALL(2, NULL, output) == -EINVAL);
    assert(CALL(2, input, NULL) == -EINVAL);
    assert(firmware_calls == 0);

    assert(CALL(2, input, output) == 0);
    assert(last_input_length == 8 && output[0] == 0x3a && output[79] == 0x3a);
    assert(freed_objects == 1);
    for (int failure = 1; failure <= 5; ++failure) {
        if (failure == 2)
            continue;
        fault = failure;
        memset(output, 0x77, sizeof(output));
        int freed_before = freed_objects;
        assert(CALL(2, input, output) == (failure == 1 ? -ENODATA : -EIO));
        for (unsigned i = 0; i < sizeof(output); ++i)
            assert(output[i] == 0x77);
        assert(freed_objects == freed_before + (failure == 1 ? 0 : 1));
    }
    fault = 0;
@EXTRA@

    pthread_t caller, remover;
    block_call = true;
    assert(!pthread_create(&caller, NULL, call_thread, NULL));
    pthread_mutex_lock(&state_lock);
    while (!entered)
        pthread_cond_wait(&state_changed, &state_lock);
    pthread_mutex_unlock(&state_lock);
    assert(!pthread_create(&remover, NULL, remove_thread, NULL));
    pthread_mutex_lock(&state_lock);
    while (!remove_started)
        pthread_cond_wait(&state_changed, &state_lock);
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += 50000000;
    if (deadline.tv_nsec >= 1000000000) {
        ++deadline.tv_sec;
        deadline.tv_nsec -= 1000000000;
    }
    while (!remove_done) {
        int result = pthread_cond_timedwait(&state_changed, &state_lock, &deadline);
        if (result == ETIMEDOUT)
            break;
        assert(!result);
    }
    assert(!remove_done); /* The device must remain alive for the in-flight call. */
    allow_return = true;
    pthread_cond_broadcast(&state_changed);
    pthread_mutex_unlock(&state_lock);
    pthread_join(caller, NULL);
    pthread_join(remover, NULL);
    assert(call_result == 0 && remove_done && !__wmi_dev);
    int calls_before = firmware_calls;
    assert(CALL(2, input, output) == -ENODEV);
    assert(firmware_calls == calls_before);
    return 0;
}
