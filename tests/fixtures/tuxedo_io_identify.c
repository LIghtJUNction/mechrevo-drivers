/* Exercise the actual identification retry helpers with safe userspace mocks. */
#define _DEFAULT_SOURCE 1
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

typedef uint32_t u32;

struct uniwill_device_features_t {
	int unused;
};

static struct uniwill_device_features_t *uw_feats;
static pthread_mutex_t uniwill_identify_lock = PTHREAD_MUTEX_INITIALIZER;
static void mutex_lock(pthread_mutex_t *lock)
{
	assert(pthread_mutex_lock(lock) == 0);
}
static void mutex_unlock(pthread_mutex_t *lock)
{
	assert(pthread_mutex_unlock(lock) == 0);
}

static struct uniwill_device_features_t mock_features;
static int interface_ready, feature_available;
static int identify_calls, feature_reads, tdp_calls;
static int sleep_first_identify;

static int uniwill_get_active_interface_id(char **id)
{
	(void)id;
	/* Identification must run under the retry lock. */
	assert(pthread_mutex_trylock(&uniwill_identify_lock) == EBUSY);
	identify_calls++;
	if (sleep_first_identify && identify_calls == 1)
		usleep(20000);
	return interface_ready ? 0 : -ENODEV;
}

static struct uniwill_device_features_t *uniwill_get_device_features(void)
{
	feature_reads++;
	return feature_available ? &mock_features : NULL;
}

static void uw_id_tdp(void)
{
	tdp_calls++;
}

/* @DRIVER@ */

static void reset(void)
{
	uw_feats = NULL;
	interface_ready = feature_available = 0;
	identify_calls = feature_reads = tdp_calls = 0;
	sleep_first_identify = 0;
	assert(pthread_mutex_trylock(&uniwill_identify_lock) == 0);
	assert(pthread_mutex_unlock(&uniwill_identify_lock) == 0);
}

static void load_order_case(void)
{
	reset();
	/* Boot-time failure: no provider yet, nothing to cache permanently. */
	uw_ensure_identified();
	assert(identify_calls == 1 && feature_reads == 0);
	assert(uw_feats == NULL && tdp_calls == 0);
	uw_ensure_identified();
	assert(identify_calls == 2 && feature_reads == 0);
	assert(uw_feats == NULL && tdp_calls == 0);

	/* Provider appears later: the next user identifies successfully. */
	interface_ready = feature_available = 1;
	uw_ensure_identified();
	assert(identify_calls == 3 && feature_reads == 1);
	assert(uw_feats == &mock_features && tdp_calls == 1);

	/* Success is cached: repeated users do no EC or provider work. */
	uw_ensure_identified();
	assert(identify_calls == 3 && feature_reads == 1 && tdp_calls == 1);

	/* R_HWCHECK_UW semantics still force a fresh identification. */
	assert(uniwill_identify() == 1);
	assert(identify_calls == 4 && feature_reads == 2 && tdp_calls == 2);
}

static void feature_failure_case(void)
{
	reset();
	interface_ready = 1;
	uw_ensure_identified();
	assert(identify_calls == 1 && feature_reads == 1);
	assert(uw_feats == NULL && tdp_calls == 0 && uniwill_identify() == 0);
	assert(identify_calls == 2 && feature_reads == 2 && tdp_calls == 0);
	feature_available = 1;
	uw_ensure_identified();
	assert(identify_calls == 3 && feature_reads == 3);
	assert(uw_feats == &mock_features && tdp_calls == 1);
}

static void *ensure_thread(void *unused)
{
	(void)unused;
	uw_ensure_identified();
	return NULL;
}

static void concurrency_case(void)
{
	pthread_t first, second;

	reset();
	interface_ready = feature_available = 1;
	sleep_first_identify = 1;
	assert(pthread_create(&first, NULL, ensure_thread, NULL) == 0);
	assert(pthread_create(&second, NULL, ensure_thread, NULL) == 0);
	assert(pthread_join(first, NULL) == 0);
	assert(pthread_join(second, NULL) == 0);
	/* Both users entered with an empty cache; exactly one identified. */
	assert(identify_calls == 1 && feature_reads == 1 && tdp_calls == 1);
	assert(uw_feats == &mock_features);
	assert(pthread_mutex_trylock(&uniwill_identify_lock) == 0);
	assert(pthread_mutex_unlock(&uniwill_identify_lock) == 0);
}

int main(int argc, char **argv)
{
	assert(argc == 2);
	if (!strcmp(argv[1], "load_order"))
		load_order_case();
	else if (!strcmp(argv[1], "feature_failure"))
		feature_failure_case();
	else if (!strcmp(argv[1], "concurrency"))
		concurrency_case();
	else
		assert(false);
	return 0;
}