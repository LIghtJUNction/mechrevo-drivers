These unmodified GPL-2.0-or-later files come from Intel thermal_daemon commit
5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a. Each full-file SHA256 is recorded in
provenance.json; the base sensor sources are reused from ../thermald_checked
with independently recorded hashes and the same commit qualification. Original
copyright and license notices remain in the frozen files.

thermald_adaptive_check.py restores these sources and applies the maintained
0001-checked-temperature.patch to the corresponding files. Preference and
periodic-cache patches do not change these adaptive inputs. The helper extracts
actual function bodies verbatim for an isolated C++ fixture. Its optional
--source-dir accepts a separately prepared source tree. The unittest can also
check every relevant file against THERMALD_PREPARED_SOURCE; absent that variable,
only this build-source equivalence test is explicitly skipped.

runtime.cpp supplies class storage and service doubles. The selected actual
sensor API, engine get_sensor_temperature, GDDV predicates/set evaluation/timers,
adaptive startup/runtime update and gddv_free/destroy_dynamic_sources execute.
GLib and libevdev object allocation/free are doubles; actual close acts on
ordinary files in the fixture directory. Aliased and separate device objects
are checked for exact release counts and their FDs must be EBADF. A second free
must be harmless. This is not full daemon startup, DBus IPC, hardware policy
execution or an installed-binary test.

The tests distinguish evaluated invalid reads from genuine no-match and valid
zero. Both invalid-startup branches must avoid aggressive/default/base-start
fallback and release resources. Ordinary no-match retains its original fallback
paths. Actual Motion, Default, time and temperature predicates verify short
circuiting: a false/time-delayed earlier predicate, an earlier matched set,
ignored temperature and a missing named sensor preserve existing behavior and
skip temperature reads. Only an evaluated failed read produces the new sentinel;
this does not assert that every sensor in skipped condition sets was validated.

Two deliberate regression mutants respectively collapse the invalid sentinel
into ordinary no-match and remove both startup cleanup calls. Original upstream
is a third negative baseline. Negative tests require exit code 1 and the exact
expected assertion; crashes, compile failures or unrelated assertions cannot
pass. Successful ordinary fallback and passive-only startup prevent blanket
rejection from satisfying the tests. Compiler and runtime children have bounded
timeouts, core dumping disabled and TMPDIR under the selected output directory.
No external downloads or additional dependencies beyond the existing C++/git
fixture tooling are required.
