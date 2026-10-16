# thermald checked-temperature candidate

Local candidate only; not installed or accepted for live operation. Package release
2 added a third patch for a reproduced virtual-sensor pthread restart window: a
previously valid cold cache could be exposed before the first new sample. The
cache is now invalidated before creating the replacement worker. The old
two-patch release 1 archive remains separate historical evidence, with that
defect explicitly unresolved. No daemon or hardware behavior is accepted.

The recipe pins official Intel thermal_daemon v2.5.13 commit
`5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a`, applies the checked-temperature
patch, the successful-preference state correction, then restart-cache invalidation. Upstream files and these
patches retain GPL-2.0-or-later provenance. The six complete upstream fixture
files have original comments and SHA-256 entries in
`tests/fixtures/thermald_checked/provenance.json`.

A failed temperature read is distinct from a valid zero. Zone callbacks wait for
all required samples; invalid reads do not advance that zone. This does not hold
all cooling devices globally: another zone can share and update a device.
Successful preference changes preserve the original protected-temperature state
in both callback rounds.

Build from this checkout with `makepkg`; `check()` requires the repository tests,
patches and prepared source inputs and fails if they are missing or inconsistent.
For offline use, `THERMALD_SOURCE_URL=file:///absolute/local/git/repository` may
replace the fetch transport; the commit identity remains mandatory. Alternate
remote URLs are rejected. No remote CI is needed. Build dependencies must already
be installed or provided privately; the recipe does not install them.

The current local production build uses `-march=x86-64-v3 -O2`, stack protection,
RELRO/NOW, and the compiler's actual C++20 default. Its candidate must be marked
`x86_64_v3`. A fresh generic `x86_64` recipe build follows its own makepkg flags;
it is not byte-reproducible with that artifact. Full bootstrap/configure/link and
DESTDIR staging were completed with real upstream tools, docs disabled, without
executing the new daemon. Release 2 requires a fresh complete compilation and link of the three-patch source;
the release 1 binary cannot represent the third fix. Repackaging skips makepkg
build/check only after those phases have been run independently against the exact
prepared source, and does not claim a second production compilation.

`python -B -m unittest discover -s tests -p 'test_thermald_checked*.py' -v`
compiles actual sensor/zone methods with sysfs/engine/callback doubles. It checks
read failures, legal zero/scale, complete multi-sensor collection, both preference
rounds, and the actual synchronous periodic callback with controlled sleep. Old
source/mutant failures must reach the specific assertions. A separate public lifecycle helper executes real pthread creation/join,
attribute cleanup, repeated activation, injected create failure and active
destruction. It gates the first new sample to distinguish stale caches of both
0 and 75,000 millidegrees; the two-patch source fails the exact stale-cache
assertion. Sysfs and cooling notifications remain doubles; production sleep
latency, whole-daemon scheduling and hardware cooling are not verified. Release 3 adds portable actual-function adaptive/GDDV startup and cleanup,
invalid-sentinel/ordinary-no-match, predicate short-circuit, virtual dependency
cycles, transactional filtering/cache and concurrent graph/parameter tests.
The full contract command requires all four test modules in package check(),
including their exact prepared-source verification. Release 4 adds portable DBus allocation-failure cleanup, output ownership and
actual handler response-format regressions; IPC and allocator boundaries remain
explicit doubles, not production GLib OOM or live-bus verification.

This differently named package provides `thermald=2:2.5.13` and conflicts with
`thermald`; replacement requires an explicit pacman transaction. Its
`2:2.5.13-2` version alone does not convert a differently named package into an automatic
upgrade plan; stock is `2:2.5.13-1.1`. It ships no install script or ALPM hook and does not enable/start
services itself. Normal system pacman hooks may reload systemd metadata; a future
explicit service start or matching D-Bus activation request could run the daemon. Existing configuration files
are pacman backup entries, which does not guarantee identical runtime behavior.
Stock reports no backup entries: the new entries do not guarantee that a conflicting
package replacement preserves existing custom configuration; preserve and verify
those files separately before any future installation.

The original signed stock package and detached signature remain separately
preserved under `Backups/projects/mechrevo-drivers/light/thermald-stock-recovery-20261016`.
Restoring package bytes and restoring active service state are different actions;
no automatic restore/activation executor is included. This candidate is unsigned
and unpublished, and has not been installed or run.


## Historical pkgrel2 artifact qualification (2026-10-16)

Fresh three-patch production compilation/link completed in 38.744 seconds.
Candidate `thermald-mechrevo-2:2.5.13-2-x86_64_v3.pkg.tar.zst` is 439,327 bytes,
SHA256 `5478e28da267a76e59c9c92928ee0b8d80e6cb0757a53ea1eb299bb23c162661`.
The new daemon SHA256 is `2c72cd7111412085e4826227b2f5bb301172ad7d6e479756df0824a1cfab15d2`.
Independent review matched nine regular payload files, 16 directories and three
metadata entries, with exact bytes/modes/root ownership. Seventeen public source
snapshots and 171 prepared source files matched the qualified production inputs.
Twelve public tests passed both author and independent runs, with the latter
using `THERMALD_PREPARED_SOURCE` set to the actual prepared source directory.
Without that environment, standalone fixture tests do not claim production-input
verification; package `check()` supplies it and rejects missing inputs.

The 48-file / 741,605-byte candidate backup was independently read back by SHA,
size, mode and owner. Actual new production tracking remains partial (six gaps,
382 unresolved events); repackage tracking is partial (four gaps, two unresolved).
The independent public test run succeeded with zero gaps and 112 unresolved
temporary-file removal events. These limits remain separate from command success.
This is an unsigned, unpublished, uninstalled artifact; no new daemon was run.


## Historical pkgrel3 artifact qualification (2026-10-16)

The complete public contract run passed 25 tests without skips in 31.547 seconds,
with actual prepared production-source inputs and unchanged before/after source
hashes. Package check() now requires all four contract modules and runs the full
`test_thermald_checked*.py` pattern; two actual Bash-function tests detect missing
adaptive coverage or selection of an incomplete test pattern.

Candidate `thermald-mechrevo-2:2.5.13-3-x86_64_v3.pkg.tar.zst` is 439,443 bytes,
SHA256 `e3c5eb728bb2eb40102d0f1c331b8244375854076ec811314a7857fa9cf4662d`.
All nine payload files, 16 directories and three metadata entries were reviewed;
payload bytes/modes/owners equal pkgrel2. BUILDINFO uses recipe SHA256
`996d222b1f70d2bc62ce183b48b567f7cd03e36fc711099fb9622c19666c8c61`.
The three production patches and daemon bytes are unchanged. This is a single
`makepkg -R -d` repackage using the independently copied and qualified real build
tree, not another production compilation or dependency check. The preceding
complete tests and exact source/binary provenance supply the skipped-phase evidence.

The 22-file / 681,059-byte candidate backup passed independent SHA/size/mode/owner
readback. Test tracking remains partial with zero gaps and 248 temporary-removal
events; repackage tracking has two gaps and four unresolved events. Raw evidence
is retained separately from successful commands. The candidate is uninstalled,
unsigned and unpublished; the new daemon has not run and no thermal benefit is claimed.


## Local pkgrel4 artifact qualification (2026-10-16)

All 32 public tests passed independently without skips in 34.407 seconds with
actual prepared production inputs. Package check() now requires all five test
modules; each missing module is rejected before Python launches. New DBus tests
cover failed reads, first/second allocation failure, invalid sensor indexes,
owned string cleanup/transfer, legal zero and actual `(ssi)`/`(u)` handler formats.
Original and three mutation controls must fail at specific assertions.

Candidate `thermald-mechrevo-2:2.5.13-4-x86_64_v3.pkg.tar.zst` is 439,458 bytes,
SHA256 `72688b7703bddae564f3737189daa68e0a85f3d67e1e2d292ab798ac28a10564`.
Nine payload files / 16 directories / three metadata entries match pkgrel2/3
payload bytes, modes and ownership. BUILDINFO recipe SHA256 is
`3e585a76b9ce850261f9f3f7b32e768c09eae271a98af9644ecae3b6d5397ffd`.
This single `makepkg -R -d` repackage did not compile/link or run a daemon again;
real three-patch production source and binary are unchanged. The 47-file /
1,013,918-byte candidate recovery archive passed both author and independent
root readback by SHA, size, mode and owner. Repackage tracking is
partial (four gaps, three unresolved events); independent 32-test tracking is
partial (zero gaps, 327 unresolved events). Raw tracking evidence remains retained.
The candidate is uninstalled, unsigned and unpublished, with no hardware claim.
