# Uniwill charging and USB EC transactions

Charging priority changes bit 7 of `0x07cc`; charging profile changes bits 4–5
of `0x07a6`. The EC update and the saved logical value form one transaction.
Without that protection, a setter can finish its hardware write, lose the CPU
to another successful setter, and then overwrite the newer restoration cache.
An adapter event or resume would subsequently restore the older request.

`uniwill_charging_lock` covers both setters and restoration's cache read plus
write. Successful writes update the logical cache and its validity; failed
reads/writes preserve the previous cache. Initialization reads the current
value before publishing sysfs. A failed initial read is logged and leaves the
cache invalid; the sysfs interface remains available for an explicit retry.
No unknown initial value is restored. Group-registration failure invalidates
that feature's saved state. Removal drains active sysfs callbacks before
clearing loaded/valid state, and a new probe starts a fresh cache lifecycle.

The lock order is caller fan command → fan state → charging → EC transaction
→ transport. Charging sysfs callbacks never acquire the fan locks. Neither
sysfs creation nor removal runs with charging held: draining a callback that
is waiting for that mutex could otherwise deadlock. Getters use a local byte
so an EC failure cannot alter the caller's output.

Both charging store callbacks copy exactly the supplied byte count and return
`-ENOMEM` on allocation failure. Their existing surrounding-whitespace parsing,
option names, successful byte-count return, and `-EIO` firmware-error behavior
are preserved. Priority remains logical 0/1; the profile helper preserves the
existing two-bit behavior, including raw value 3, without guessing its meaning.

USB powershare uses bit 4 of `0x0767`, the same register used by RGB mode.
Its read/modify/write now uses `uniwill_update_ec_ram`, retaining the other bits
under the provider's shared transaction mutex. The RGB path must also use the
shared helper for the pair to be protected in both directions.

`test_uniwill_charging_runtime.py` contains 27 actual-C tests with mocked EC/sysfs
services and fixed pthread interleavings. The cache race pauses after the
mock EC transaction has released its lower mutex but before the charging caller
updates its cache; the charging lock is therefore tested independently.
Six additional tests combine actual charging bodies with the actual shared
EC-provider bodies. They exercise every previous byte and input-byte combination,
error propagation/output preservation, and USB's use of the shared update API.
The test labelled RGB performs a competing bit update through that API; it does
not replace a separate integration test of the actual RGB LED callback.

These are software tests, not charging/USB/keyboard hardware acceptance.
This change does not add PM admission control to every independent sysfs path,
does not serialize firmware's autonomous updates, and does not change AC auto
boot, lightbar, battery raw-counter semantics, or any hardware limit.
