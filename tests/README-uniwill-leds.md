# Uniwill keyboard LED callback regression tests

Run from the repository root:

```sh
ulimit -c 0
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test_uniwill_led_runtime.py' -v
```

This suite extracts the actual white/RGB setter functions, their EC helpers and
both LED device initializers from `src/uniwill_leds.h`. It compiles these unchanged
with mock EC transport and the unmodified LED core functions recorded in
`fixtures/linux_led_core_v7_2_9.c`. The kernel service doubles model bit flags,
queued work and timer dispatch; they do not constitute a real kernel workqueue,
lockdep, LED trigger or hardware test.

The LED core excerpts come from official Linux stable `v7.2.9`, commit
`5fce161649b4d779d1b76d9fcd52dc77779774b8`,
[`drivers/leds/led-core.c`](https://github.com/gregkh/linux/blob/5fce161649b4d779d1b76d9fcd52dc77779774b8/drivers/leds/led-core.c).
The full original file has SHA-256
`97dffa18ffcc714ad0892d4e049ea5dd4a2f3df3c6203ee227f1177237db6946`.
The excerpt retains its GPL-2.0-only license and original author attribution.
This source version matches the upstream version component of the local
`7.2.9-arch1-1` kernel, but does not reproduce Arch-specific patches.

The regression is a concrete API mismatch: `brightness_set` must not sleep,
but Uniwill setters call EC transport that acquires a mutex and may allocate
with `GFP_KERNEL` or wait with `msleep`. A normal software blink timer reaches
this callback through `led_set_brightness_nosleep()`. The repaired initializers
use `brightness_set_blocking`, so that entry point queues work. Both the actual
core timer callback and direct nosleep dispatcher are exercised with an atomic
context flag; an EC call before queued work is run fails an assertion. Work is
explicitly drained later in simulated process context.

Thirty-three tests cover both white and RGB callbacks, the existing version branch
for multicolor `max_intensity` (6.10 and 7.2 preprocessing), status/mode bit
preservation, exact successful EC ordering, every RGB channel's 256 input values,
the existing RGB-black behavior, and each EC failure boundary with five distinct
negative errnos. RGB-black continues to request hardware brightness zero while
retaining the requested software brightness. Successful earlier EC writes are
left in the mock EC after a later failure: this change provides no rollback of
partial RGB writes.

White immediate-brightness read failure now stops before any dependent write;
its workaround-write failure stops before the status read/write. These failures
and the existing RGB/status helper errors are returned by the blocking callback.
The actual LED core work handler reports the original errno, and its synchronous
entry point returns it. Successful and failed helper sequences enter through that
synchronous core API; the core owns the requested-brightness cache. The LED core can cache the requested brightness before
calling the driver; a transport error does not make that cache confirmed
hardware state and this patch does not roll it back.

Two additional interleaving cases inject a newer atomic nosleep request during
an older worker's first EC transaction. The old worker must preserve the new
requested brightness, and a second worker must apply it. Keeping the historical
callback cache assignment after switching to blocking callbacks fails both
cases on both preprocessing versions (four failing subtests). The repaired
setters leave the cache entirely to the LED core. These cases cover that precise
interleaving, not general LED state coherence or concurrent color restoration.

For baseline sensitivity, set `UNIWILL_LEDS_HEADER` to an older driver's header.
The old `void` callbacks are extracted without rewriting; only a test invocation
wrapper chooses the available callback type. Using base
`a929f9ce143316b3b18d5f3fbf9cc54f0c1122e2` yields twenty-five failing tests on each of the
two preprocessing versions (50 failing subtests), including the actual timer's
atomic EC access. Successful behavior checks also run against both old and new source. Core dumps are disabled for assertion failures. No physical backlight,
EC register, software blink trigger, suspend or reboot is used by these tests.


## Core PM and initialization lifecycle

The two LED devices also use `LED_CORE_SUSPENDRESUME`. The default LED workqueue
is ordered but is not freezable. Without the flag, deferred brightness work can
capture the old low status bits, outlive parent suspension and write those bits
back after the parent sets the backlight-disable bit. The child LED core PM path
sets `LED_SUSPENDED`, schedules temporary OFF and flushes brightness work before
the parent disables backlight. The temporary OFF retains the user's requested
brightness. Parent resume precedes child resume, which queues the latest cached
request. A new request while suspended updates the cache without EC access.

`fixtures/linux_led_pm_v7_2_9.c` contains four unchanged functions from official
Linux v7.2.9 `led-class.c` at the same pinned commit:
`led_classdev_suspend`, `led_classdev_resume`, `led_suspend` and `led_resume`.
The full original file has SHA-256
`50913ab981c8431a834779b4d0ad9673393629239415199818b3cc9945c44dad`.
Its GPL license and John Lenz / Richard Purdie attribution are retained.

The fixture executes those actual child PM functions and the actual parent
backlight read/modify/write helper. Parent-child scheduling and failure unwinding
are an explicit hierarchy model, not the full kernel DPM engine or the complete
parent PM callbacks. The modeled order and failure policy follow official
`drivers/base/power/main.c`: an already-suspended LED child resumes after parent
suspend abort; child resume also runs after a parent resume error. Core LED PM
callbacks return zero even if asynchronous OFF or restore EC work fails; that
errno is reported by the work handler. The tests preserve this policy, retain
the requested cache and do not claim failed parent resume enabled the backlight.

The actual LED `init`/`remove` functions are extracted too. Tests cover every
successful feature byte, negative errors from both feature reads and the initial
feature write, zero/failed barebone reads, registration failures, and legal
remove/rebind cycles. Each registration resets lifecycle flags, then adds hardware
notification capability for that detection. Detection state and the two-level
white limit no longer leak from a previous binding. A requested five-level value
4 is retained through child suspend, cleared to OFF by the modeled core unregister
sequence and verified within the new two-level limit after rebind; registration
and resume themselves do not clamp arbitrary old cache values.

A failed LED registration keeps the detected backlight type available, but its
direct restore entry point now does no EC work until initialization succeeds.
Late direct restore after removal is also ignored. Successful restore retains
its prior EC ordering and error policy, including independent brightness restore
after a failed RGB-color restore. Kernel registration/unregistration services are
mocked; the full core unregister path was separately checked to establish its
stop-blink, OFF-request and flush behavior.

The intermediate `30ecaa7ea37356a954dfdbb48ae8495fa134c9d6` header already has blocking
callbacks and request-cache ownership fixed, but lacks core PM/lifecycle changes.
Running these same 33 tests against that header produces 24 failing subtests;
the queued-RMW case specifically detects the parent disable bit being cleared.
This distinguishes the deferred-work PM regression from the original sleeping
atomic callback bug.

The independent maintenance LED tree and canonical baseline still contain the
old parent backlight helper returning `void`; its unchecked EC read is repaired
separately by the parent PM/probe patch. The harness compiles this helper unchanged
and permits only `maybe-uninitialized` to remain a warning for that legacy helper
configuration. All compiler output is retained. With the fixed `int` helper in
the combined maintenance tree, compilation uses strict `-Werror` throughout.
Neither this exception nor the hierarchy model is a hardware PM validation.
