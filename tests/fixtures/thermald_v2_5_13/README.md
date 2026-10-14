# Pinned thermald controller inputs

These GPL-2.0-or-later snapshots come from Intel thermal_daemon `v2.5.13`,
commit `5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a`:
<https://github.com/intel/thermal_daemon/tree/v2.5.13/src>.

The four headers preserve upstream class declarations, inline methods and
preference constants byte for byte. The three C++ files retain only the exact
function definitions used by the regression fixture, plus their original
license headers. `provenance.json` records whole upstream and snapshot SHA-256
hashes and the hash of each selected function. The overloaded RAPL getter has
one combined hash for both original definitions in source order.

`tests/test_thermald_ceiling_runtime.py` validates these hashes before compiling.
With `THERMALD_PINNED_SOURCE_DIR=/path/to/thermal_daemon`, it additionally verifies
whole source hashes and compares every selected definition against that frozen
checkout. The ordinary repository suite is self-contained and needs no network.

The executable uses actual RAPL initialization, range clamps, setters, cooling
controller and sequential trip selection. Sysfs, PPCC discovery, power-meter
observations, engine/sensor services and the next cooling device are in-memory
doubles. PID methods and
critical/shutdown paths abort if entered; they are not validated by this suite.
Registration of process-exit restoration is also outside the fixture. Parameter
setters model XML range application directly; the XML parser and
`add_replace_cdev()` are not executed here.

The tests establish why both range endpoints need updating: with original
140 W initialization, upstream's 50 percent rule leaves a 70 W cooling floor.
Setting only the 55 W upper endpoint prevents a 40 W cooling request and blocks
sequential progression. A paired 55 W / 27.5 W range permits further cooling and
progression to the next cooling device. The 27.5 W value is the same upstream
range formula applied to the new upper endpoint; it is not an EC register value
or a claim about a vendor hardware minimum.

Counterexamples document unsupported fallback and initially-disabled states.
Upstream also caches the requested state after a failed write, so production
must independently read back the actual sysfs value. These tests do not prove
daemon startup, firmware behavior, PID control, process-exit restoration,
suspend/resume behavior or physical thermal safety.
