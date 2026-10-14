# Uniwill probe and backlight error handling

`test_uniwill_probe_runtime.py` extracts the complete production probe, remove,
filter and prepare-remove functions, the actual fan-start/quiesce/lock helpers,
and the actual keyboard-backlight read/modify/write helper. Only the external
EC transport, filter registration, resource and workqueue services are mocked.
The original source functions are not rewritten to make the fixture pass.

The 6.10, 6.13, 6.14 and 7.2.9 macro builds exercise both filter API signatures
and both platform-remove return types. These are userspace C builds, not old
kernel module builds or hardware tests.

The EC cases cover each curve read/write failure, the initial fan-mode write,
manual enable and second-fan detection failures, three distinct errno values,
the limited manual-disable cleanup and a separate cleanup failure, and retry
after every critical failure. They check that fan exports are closed and work
is drained before a failed probe returns, without publishing device resources
or a filter. A curve or failed manual-enable write has no guessed rollback.
After manual enable succeeds and second-fan detection fails, the probe only
attempts the same manual-disable write already defined by remove. Its failure
is logged independently and never replaces the original probe error. Partial
EC writes therefore must not be described as fully restored hardware state.

Backlight tests exercise all 256 original bytes and all 256 enable argument
bytes, plus all 256 initial bytes against all 256 current bytes at remove.
Only the active-low enable bit is restored; unrelated current bits survive.
A failed read causes no dependent write, errors remain visible, and a new
probe clears the saved-state sentinel even when its first EC operation fails.
Optional backlight failures keep the keyboard bound and are logged. An unknown
initial state is never restored using a fabricated value.

`UNIWILL_PROBE_BASELINE=/absolute/tree` runs this same fixture against an older
tree. A wrapper adapts only invocation of the older void backlight helper to
the new test return convention; all extracted driver bodies remain unchanged.
The f178aed baseline fails 13 new behavior tests (52 version subtests), while
the remaining 10 tests pass. For an explicitly selected older tree, the known
`maybe-uninitialized` warning is printed and allowed so the old source can
reach the runtime assertions. Current source keeps strict `-Werror` builds.
Logs and build records are retained under
`.local-tests/next-20261015/probe-validation` in the original project.

No EC fault injection, real unbind/rebind, backlight restore, suspend/resume or
physical fan test is performed by this fixture. PM callback error propagation
and adapter-event serialization are validated by the separate PM change and
the final combined test run.
