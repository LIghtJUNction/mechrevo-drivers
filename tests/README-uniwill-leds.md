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

Seventeen tests cover both white and RGB callbacks, the existing version branch
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
`a929f9ce143316b3b18d5f3fbf9cc54f0c1122e2` yields twelve failing tests on each of the
two preprocessing versions (24 failing subtests), including the actual timer's
atomic EC access. The five successful behavior tests pass on both old and new
source. Core dumps are disabled for assertion failures. No physical backlight,
EC register, software blink trigger, suspend or reboot is used by these tests.
