# Custom profile error regression tests

Run `PYTHONDONTWRITEBYTECODE=1 python3 tests/test_uniwill_custom_profile.py`.
Use `TMPDIR` to select the temporary build directory. The compiler (`CC`, or
`cc` by default), Python 3 and standard C library are the only dependencies.
These tests never access hardware or require a loaded driver.

The harness extracts the actual `uniwill_set_custom_profile_mode`,
`uniwill_keyboard_probe` and `uniwill_event_callb` functions from the header.
The probe also uses the actual fan-start/quiesce/unlock helpers and checks the
actual i8042 installation-state tail. It replaces EC transport, delays, input
and resource registration with test
doubles. Sixteen tests cover all 256 initial register bytes in both successful
helper paths, unsupported-feature no-ops, read/reset/set failures with three
errno values, unchanged initialization ordering, and independent adapter
restoration. The full actual probe function is compiled; resource registration
is observed through doubles rather than exercising the kernel resource APIs.

To check sensitivity against an older revision, point
`UNIWILL_KEYBOARD_HEADER` at its `src/uniwill_keyboard.h`. The harness adapts the
old void helper only at the test invocation boundary. It does not rewrite its
body or the actual probe/callback. The prior maintenance `6a924ac` executes the
suite with seven failures and nine passes.

Failure policy: the helper stops its dependent EC sequence and returns the
original errno. Probe and adapter callbacks report this optional feature's
failure and continue their existing independent initialization/restoration.
A successful reset followed by a failed set can leave bit 6 clear; this is not
a hardware rollback. The patch does not guarantee custom profile mode was
applied after transport failure. It does not serialize adapter and PM accesses
or establish firmware support on any additional model. The current GM6PX0X
feature flags leave this feature disabled. Tests force the capability
only in doubles to exercise the guarded code. No hardware with this feature
enabled was available for physical validation.
