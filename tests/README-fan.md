# Uniwill fan state regression tests

Run from the repository root with Python 3, a C11 compiler and pthread support:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test_uniwill*.py' -v
```

The tests extract the production C functions and compile them without rewriting
their bodies. EC memory, workqueue scheduling, kernel mutexes and device resource
operations are represented by mocks. The concurrency fixture uses pthreads and
barriers at selected EC accesses to force conflicting operations into a defined
order. It checks the resulting EC bytes, cached targets and queued work; it does
not depend on random scheduling. A contender can either finish or block before
the first operation is released, so an unlocked old implementation can run and
fail the state assertions rather than merely time out.

The fan runtime and concurrency fixtures default to `.local-tests` in this
checkout; `UNIWILL_TEST_TMPDIR` selects a dedicated directory. Set `TMPDIR` as
well to redirect the standalone custom-profile fixture. The runners remove
their temporary builds and disable subprocess core dumps, including for expected
baseline assertions.

`UNIWILL_FAN_HEADER` selects a frozen production header for the concurrency test.
When it lacks the new mode helpers, also set `UNIWILL_FAN_IO_BASELINE` to the
matching frozen `src/tuxedo_io/tuxedo_io.c`. The compatibility wrapper then calls
that original profile read/modify/write function. It suppresses only warnings
from the old unsigned comparisons; it does not change the function body. The
new reprobe helper case is skipped because it has no old equivalent. Other
concurrency cases use the old functions directly.

The routing fixture separately extracts the actual profile function and the
`W_UW_MODE` case. Its helper stubs check which serialized API is called and with
which masks/values. The fan fixtures test those helpers' state and EC behavior.

These tests do not load a kernel module or touch hardware. They cannot validate
physical fan response, EC firmware behavior, suspend/reboot, thermal limits or
noise. A successful legacy manual request may only have queued the first worker;
it does not guarantee that the asynchronous EC writes have completed.

The binding fixture runs the production common input/init/remove functions,
Uniwill interface add/remove and event callback, and WMI probe. It also executes
the real fan-start helper and the initial fan-start portion of keyboard probe.
The remainder of probe is deliberately omitted: these tests cover bind failure
propagation and rollback, rather than LED/battery/sysfs initialization. Set
`UNIWILL_BINDING_BASELINE` to a frozen repository tree to combine its old outer
binding functions with the new fan-start failure path; this checks the cleanup
regression introduced when the initial EC failure can abort probe.

The deferred-key fixture extracts the real common removal function, Uniwill
removal/filter/worker/preparation functions, and the preparation hook from the
production driver initializer. Controlled pthread schedules exercise queued and
running key work during common removal and direct platform unbind. They require
filter shutdown and synchronous work cancellation before the input pointer is
cleared or the input device is freed, while keeping input teardown before its
platform parent. No-input initialization and absent filters are also covered.
`UNIWILL_KEY_WORK_BASELINE` selects a frozen tree for regression sensitivity.
These cases do not simulate WMI bus notification lifetime; that is a separate
kernel source-review assumption for standard WMI unbind.

The PM/adapter fixture extracts the complete actual probe, adapter callback,
fan/worker helpers, prepare/remove, suspend and resume functions. Fifteen cases
use controlled pthread schedules and EC/resource doubles to cover adapter versus
suspend, resume and removal in both orders; late adapter events in AUTO and
manual sleep; all exported command gates; probe/failure/remove/reprobe; and
failed suspend/resume recovery without requiring a second PM callback. The
separate ready and suspended state prevents a late cable event from setting
custom-profile bit 6 again after a successful suspend. Backlight failures are
returned from PM, and failed resume preserves manual targets while leaving
ordinary commands available for recovery after the callback returns. A worker
must not run or requeue before probe completes, during sleep or after early
remove preparation. These tests use the full source functions with mock
services; they do not establish real firmware, WMI/LED core lock ordering,
suspend or hotkey behavior on a physical device.

The !61 lifecycle contribution supplies 131 tests: 37 fan/fault/boundary, 12 deterministic
fan concurrency, 4 ioctl routing, 13 binding, 11 deferred-key, 23 complete probe
and backlight, 16 custom-profile, and 15 PM/adapter cases. The probe cases also
run across four API version macro selections. The independent custom-profile
change is merged as a prerequisite so the adapter and failed-suspend paths can
retain its original-errno handling. This TDP transaction branch merges the
independent !59 usercopy/capability prerequisite and adds IO/provider coverage;
its complete suite contains 201 tests. Clevo ioctl changes remain independent.
