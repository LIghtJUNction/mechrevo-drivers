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

The historical pre-transaction sensitivity results below were recorded with the
earlier 33-test harness. For that baseline, `UNIWILL_LEDS_HEADER` selected the older
driver header.
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
Before the transaction follow-up, the 33-test harness recorded 24 failing
subtests against that header; its queued-RMW case detected the parent disable
bit being cleared.
This distinguishes the deferred-work PM regression from the original sleeping
atomic callback bug.

The canonical pre-transaction input retains the old parent backlight helper
returning `void`. This follow-up converts that helper to return the atomic EC
update result as `int`. The parent PM/probe patch also repairs this helper and
its callers separately; the callers' existing error policy is unchanged here.
The harness compiles old sensitivity inputs without rewriting their helper,
adapting its void return only at the test invocation boundary and permitting
only `maybe-uninitialized` to remain a warning in that legacy configuration.
All compiler output is retained; the current helper uses strict `-Werror`.
Neither this exception nor the hierarchy model is a hardware PM validation.

## Driver transaction and firmware event follow-up

The LED domain mutex serializes each driver's complete white/RGB apply, direct
restore, firmware notification, and readiness publication. It never surrounds
LED core registration, unregistration, work flushing, or cancellation. In
particular removal closes direct restore/notification before unregister, while
the unregister OFF callback remains usable and acquires the mutex normally.
Normal init/remove calls still rely on their existing outer device/interface
serialization; the domain mutex does not make arbitrary simultaneous exported
init/remove calls a supported lifecycle.

Brightness and parent enable use the atomic EC update helper for their shared
`0x078c` byte. The brightness update preserves the existing low nibble and
replaces the full upper nibble, including its apply bit. RGB apply uses the same
helper for `0x0767`, which also contains USB powershare's independent bit. The
helper holds a transaction mutex across the actual read and dependent write;
public individual EC reads/writes participate in that same mutex. None of these
changes add EC retries or roll back partial color writes.

Firmware brightness events are consumed for an initialized EC-controlled LED
without touching the EC or requested cache while the child is suspended. EC
read errors and failed RGB compensation leave the requested cache and hardware
notification untouched. The suspended flag is checked again after a sleeping
read/compensation so a waiting event does not overwrite the desired state from
a completed LED-core temporary OFF. This is a defensive check, not an atomic
contract with every LED-core flag/cache writer. Direct RGB restoration now uses
the existing black special case: all-zero color channels request hardware
brightness zero without writing EC default-color commands or changing the
cached requested brightness. Nonblack restore retains the prior independent
brightness attempt after a color error.

The existing 33-test core fixture now extracts the real public EC wrappers and
atomic update helper, with private transport service doubles. The older probe
and parent-PM fixtures instead provide an explicit update service double to
preserve their existing probe/PM assertions. Shared-register and driver-domain
concurrency are tested separately by `test_uniwill_led_transactions.py`, which
runs the real foundation and LED functions with pthread schedules. These tests
do not establish that LED core's requested brightness, its RGB intensity tuple,
or firmware state form one globally atomic snapshot. No physical backlight,
register, trigger, sleep, reboot or runtime kernel lockdep is used.

Run the 18 follow-up tests with:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test_uniwill_led_transactions.py' -v
```

Both preprocessing versions run the same actual source. The fixed pthread
schedules cover both orders of brightness/enable updates for all 256 original
status bytes and both enable values; full RGB callback/restore transaction
sequences with distinct immutable color inputs; every white/RGB EC failure and
subsequent recovery; and RGB-black restore at every requested level and original
status byte. Notification checks include successful existing white/RGB mapping,
failed reads/compensation, uninitialized/absent/uncontrolled state, a completed
child suspend, and child suspend arriving during the notification read or RGB
compensation. The latter runs the actual frozen child suspend and queued OFF
callback, verifies that it waits for the domain mutex, then resumes the latest
requested brightness. Registration verifies readiness is still unpublished and
our mutex is unlocked while a core callback runs. Unregister is a service model
that requests OFF through actual core queueing and drains its actual callback;
it is not execution of the full kernel unregister implementation.

For this upstream follow-up's sensitivity, select the
`261a3cdc3ee0150997382c6e9675e813de7af6d7` LED header with `UNIWILL_LEDS_HEADER`
and its keyboard header with `UNIWILL_LED_KEYBOARD_HEADER`.
That commit is the real merge of the original LED branch `ee8ec494` and EC
foundation `59e31e6`; the local-maintenance equivalent used `2dae697` inputs. The new EC foundation remains the actual compiled
API from the current tree; this isolates the old LED/parent implementation
above it rather than claiming a build of the complete old driver tree. Twelve
of the 18 methods fail as runtime assertions on both preprocessing versions
(24 failing subtests), while six methods pass. These failures include both
confirmed stale-register/cache errors and the new coordination contract; they
are not a count of twelve distinct pre-existing vulnerabilities. The earlier
33 core tests still pass with those same old LED and parent inputs, confirming
that their established successful behavior and error policy remain intact.
