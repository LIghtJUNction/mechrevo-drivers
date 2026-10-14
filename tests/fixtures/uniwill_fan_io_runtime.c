/* Production C is injected verbatim; the two exported helpers are mocked here. */
#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
/* @PROFILE_ENUM@ */
/* This fixture validates serialization for supported public profiles. The
 * ioctl fixture separately covers every advertised capability combination. */
static struct {
    int uniwill_profile_v1_two_profs;
    int uniwill_profile_v1_three_profs;
    int uniwill_profile_v1_three_profs_leds_only;
} features = { .uniwill_profile_v1_three_profs = 1 }, *uw_feats = &features;
static int mode_calls, update_calls, helper_status;
static u8 mode_value, clear_mask, set_mask;
static int uw_set_mode(u8 value)
{
    ++mode_calls;
    mode_value = value;
    return helper_status;
}
static int uw_update_mode(u8 clear, u8 set)
{
    ++update_calls;
    clear_mask = clear;
    set_mask = set;
    return helper_status;
}
static u32 copy_from_user(void *to, const void *from, unsigned long size)
{
    memcpy(to, from, size);
    return 0;
}
/* @PROFILE_FUNCTION@ */
#define W_UW_MODE 1
static long raw_mode_ioctl(unsigned long arg)
{
    u32 argument, copy_result = 0;
    switch (W_UW_MODE) {
/* @MODE_CASE@ */
    }
    (void)copy_result;
    return 0;
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    if (!strcmp(argv[1], "profiles")) {
        enum uw_perf_profiles_v1 profiles[] = {PROFILE_POWERSAVE, PROFILE_ENTHUSIAST, PROFILE_OVERBOOST};
        const u8 bits[] = {0xa0, 0x00, 0x10};
        for (unsigned int i = 0; i < 3; ++i) {
            assert(uw_set_performance_profile_v1(profiles[i]) == 0);
            assert(update_calls == (int)i + 1 && mode_calls == 0);
            assert(clear_mask == 0xb0 && set_mask == bits[i]);
        }
    } else if (!strcmp(argv[1], "invalid")) {
        assert((int)uw_set_performance_profile_v1(0) == -EINVAL);
        assert((int)uw_set_performance_profile_v1(4) == -EINVAL);
        assert((int)uw_set_performance_profile_v1(UINT32_MAX) == -EINVAL);
        assert(update_calls == 0 && mode_calls == 0);
    } else if (!strcmp(argv[1], "errors")) {
        const int errors[] = {-ENODEV, -EIO, -ETIMEDOUT};
        for (unsigned int i = 0; i < 3; ++i) {
            helper_status = errors[i];
            assert((int)uw_set_performance_profile_v1(PROFILE_POWERSAVE) == errors[i]);
            assert(update_calls == (int)i + 1 && mode_calls == 0);
        }
    } else if (!strcmp(argv[1], "mode")) {
        for (u32 value = 0; value < 256; ++value) {
            u32 argument = 0xabcd0000 | value;
            assert(raw_mode_ioctl((unsigned long)&argument) == 0);
            assert(mode_calls == (int)value + 1 && mode_value == value && update_calls == 0);
        }
    } else {
        assert(!"unknown test case");
    }
    return 0;
}
