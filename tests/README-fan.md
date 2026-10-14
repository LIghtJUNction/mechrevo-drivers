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

Temporary files are created in the system temporary directory and removed by the
test runner. Set `UNIWILL_TEST_TMPDIR` to use an existing dedicated directory.
Subprocess core dumps are disabled, including for expected baseline assertions.

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
