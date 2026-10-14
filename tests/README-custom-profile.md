# Custom profile error regression tests

Run `PYTHONDONTWRITEBYTECODE=1 python3 tests/test_uniwill_custom_profile.py`.
Use `TMPDIR` to select the temporary build directory. The compiler (`CC`, or
`cc` by default), Python 3 and standard C library are the only dependencies.
These tests never access hardware or require a loaded driver.

The harness extracts the actual `uniwill_set_custom_profile_mode`,
`uniwill_keyboard_probe` and `uniwill_event_callb` functions from the header.
It replaces EC transport, delays, input and resource registration with test
doubles. Sixteen tests cover all 256 initial register bytes in both successful
helper paths, unsupported-feature no-ops, read/reset/set failures with three
errno values, unchanged initialization ordering, and independent adapter
restoration. The full actual probe function is compiled; resource registration
is observed through doubles rather than exercising the kernel resource APIs.

This branch integrates the fan lifecycle changes and extracts the actual
fan-start helpers required by the complete probe. Its fixtures therefore require
a header with those helpers. `UNIWILL_KEYBOARD_HEADER` can select a frozen header
with the same lifecycle API. For the independent custom-profile patch's baseline
sensitivity, use its tests at `15cc292` against upstream `a929f9c`: those tests
execute with seven failures and nine passes. That standalone harness adapts the
old void helper only at the invocation boundary, without rewriting the helper,
probe or callback bodies.

Failure policy: the helper stops its dependent EC sequence and returns the
original errno. Probe and adapter callbacks report this optional feature's
failure and continue their existing independent initialization/restoration.
A successful reset followed by a failed set can leave bit 6 clear; this is not
a hardware rollback. The patch does not guarantee custom profile mode was
applied after transport failure. The independent custom-profile patch does not
serialize adapter and PM accesses or establish firmware support on any additional
model. This fan lifecycle branch adds adapter/PM serialization and gating; the
separate PM fixture covers that interaction. No TUXEDO hardware was available
for physical validation.
