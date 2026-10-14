# Uniwill TDP command transactions

`test_uniwill_tdp_transaction.py` compiles the actual `uw_set_tdp()` IO helper,
its min/max and public-profile helpers, the complete keyboard fan-provider
section, and the actual prepare-remove, remove, suspend and resume functions.
The extracted provider includes its real locks, active gate, mode APIs, fan
APIs and worker. EC transport, workqueue services, feature discovery and
unrelated resource teardown are userspace mocks. No driver implementation is
copied into the fixture. The outer ioctl and its user-buffer handling remain
covered by `test_uniwill_runtime.py`, not these new tests.

Run from the project root:

```sh
TMPDIR="$PWD/.local-tests/tdp-tmp" \
UNIWILL_TEST_TMPDIR="$PWD/.local-tests/tdp-tmp" \
python -m unittest discover -s tests -p test_uniwill_tdp_transaction.py -v
```

Create the selected temporary directory before running. The 23 test methods
run both normal and `DEBUG` C builds with pthreads, warnings treated as errors,
disabled core dumps and bounded subprocess deadlines.

The deterministic schedules pause the first TDP request immediately before
the final `0x0783` write commits. A second actual profile, raw-mode, full-fan,
suspend, prepare-remove, remove or TDP request must report waiting on the
command lock, rather than completing. No second actor's EC operation may
precede the first commit. The commit records the device's ready, suspended
and removed states and, when required, the OVERBOOST mode bits. Two competing
TDP requests must retain complete command order and end at the later value.
The mutex mock also records premature command-lock releases within a
successful actual IO request, before its final write attempt. This runtime
observation catches dropping the prerequisite lock and reacquiring a separate
lock around the final write, which the final-write pause alone cannot detect.

Reverse-order cases complete actual suspend, prepare-remove, remove or
lifecycle start first. For both custom and noncustom feature configurations,
valid TDP requests must then return `-EBUSY` or `-ENODEV` without any EC access.
Actual resume completion must reopen the request. Read, mode-write and final
TDP-write failures preserve five distinct negative errors and stop dependent
operations. A successful mode change followed by failed TDP is not rolled
back. Clearing each fault and retrying the same request, followed by a raw
mode request, verifies both lock release and a usable active gate. The direct
provider rejects invalid indices before EC access. Existing IO tests cover
all three addresses, limits, unsupported models and double-PL4 encoding.

For sensitivity checks, `UNIWILL_TDP_SOURCE_ROOT=/absolute/source/tree`
selects all production input files from that source tree. Against old
upstream dependency baseline `55086e338bbe108ee4a086c9929026efbc9fabc2`,
which merges the still-independent !59 and !61 changes, the fixture executes
the actual old IO and provider; it does not supply a fake new TDP helper.
Only the direct new-API index case is explicitly skipped. The old source fails
32 subtests (16 methods across the two builds). A mode-unlock-relock mutation
of the fixed upstream source fails 22 subtests (the 11 interleavings across both builds); this
demonstrates the command-boundary observation is necessary.

These tests force feature flags and power-limit arrays in mocks. They do not
add model support, establish safe power limits or validate firmware behavior.
The available GM6PX0X remains unsupported for TDP writes; no actual TDP write
was performed. The new exported helper requires a matching keyboard provider
and IO module. Installing both on disk does not update an already loaded
provider; the planned reboot is a separate activation and hardware check.
