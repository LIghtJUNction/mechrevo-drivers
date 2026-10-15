# Uniwill EC update transaction

`uniwill_update_ec_ram(address, clear_mask, set_mask)` performs one successful
read followed by `(old & ~clear_mask) | set_mask` under a private provider mutex.
Set bits win when masks overlap. It returns the original transport status; a
failed read cannot issue a write. A failed write is not retried or rolled back:
a failed physical transaction may still have changed hardware.

All public `uniwill_read_ec_ram` and `uniwill_write_ec_ram` operations use the
same mutex. The new helper calls private unlocked implementations while holding
it, so it does not recursively lock the public wrappers. Existing missing
interface and read-pointer delegation behavior is preserved. Lock order is
caller domain locks, then the private EC transaction mutex, then existing WMI
or direct-EC transport locks. No generic lock/unlock functions are exported.

The actual-C test extracts these five complete provider functions, the real
interface structure/typedefs, and mutex declaration without rewriting them.
Transport memory and kernel mutex services are explicit userspace pthread
doubles. It checks all 16,777,216 initial-byte/clear-mask/set-mask combinations,
u16 address endpoints, original read/write errors, absent/error interface
pointers, NULL/error read-pointer validation, and failure recovery. It also
models a failed write with a hardware side effect to ensure no guessed rollback.

Deterministic pthread cases pause the first transport operation after sampling
or writing, start a competing public operation, and verify that it is blocked.
The recorded trace must contain complete update read/write pairs before the
next read, write or update can access transport. Disjoint and overlapping
updates are checked in both orders. A scheduler hook after a standalone read's
unlock exposes lost updates in an intentionally broken implementation that
uses separately locked public read/write calls. The unchanged complete
transaction does not reach that hook between its read and write. Nonrecursive
mutex semantics are asserted rather than letting a broken test hang.

This is a single-register atomicity primitive among these public provider
operations. It does not serialize firmware, external direct transport users,
`uniwill_wmi_evaluate`, interface registration/removal, two-register u16 reads,
or a complete write/retry/readback sequence. It does not replace LED, charging
or fan multi-operation/cache transactions. No hardware or module is accessed by
the runtime tests. Compilation and pthread execution are not physical EC or
scheduler/lockdep validation.

The read retry helper has a separate invalid-budget guard: retries <= 0 returns
-EINVAL before transport or mutex access. Actual-source cases also retain first
success, transient failure recovery, strict retry-budget exhaustion, final
transport errno, output preservation, absent interface, and NULL delegation.
These attempts remain independent public read transactions.

## Fn-lock caller integration

`test_uniwill_fn_lock.py` combines the actual Fn-lock getter/setter and sysfs
functions with all five actual provider read/write/update functions, the real
address/mask definitions, and the existing pthread/transport scheduler doubles.
The setter updates only the existing `0x10` bit at `0x074e` within one shared
transaction; it does not define a new register or change capability detection.
Internal nonzero inputs retain their boolean semantics, while sysfs still accepts
only 0/1. The parsing double is not a replacement test of kernel `kstrtoint`.

Twelve cases cover all 256 register bytes, read/write failures, partial failed
write side effects, NULL getter output, missing interface, sysfs validation and
errno, and both orders of a competing public bit update or full-byte write.
The fixed schedules expose the old setter releasing its lock after read: the
competing operation completes before the stale Fn write, losing an unrelated
bit or overwriting a completed write. The old setter fails both forward-order
cases; the transaction implementation preserves complete-operation traces.
`UNIWILL_FN_LOCK_HEADER` selects a historical header without replacing its bodies
for a red regression run. These are simulated provider races, not proof that
this machine has experienced a physical lost-update incident. Firmware remains
outside this provider mutex, and the tests do not use real EC writes.

## AC auto-boot caller integration

`test_uniwill_ac_auto_boot.py` extracts the actual AC auto-boot setter/getter,
sysfs functions, cache declaration and all five provider functions. It uses the
existing address `0x0726` and bit 3; the setter retains its internal u8 low-bit
input semantics and only updates the existing bool cache after transport success.
Sysfs continues to accept only 0/1 and translate transport failures to `-EIO`.
The parsing double does not replace testing kernel `kstrtou8`.

Twelve cases cover all 256 initial register bytes with all 256 u8 inputs,
getter/output behavior, unchanged cache after failures, missing interfaces,
read/write errors, a failed write with a hardware side effect, sysfs behavior,
and both operation orders with another-bit update or full-byte write. The
actual-source pthread schedules expose the old split read/write setter losing
an unrelated update or overwriting an already completed write.
`UNIWILL_AC_AUTO_BOOT_HEADER` selects historical source for red regression runs.
The cache currently has no readers; these tests do not claim that its write
outside the provider transaction lock is synchronized with concurrent setters.

This change does not expand the AC auto-boot capability whitelist. GM6PX0X does
not match that whitelist, so these simulated provider tests and module builds
are not physical AC auto-boot validation on this machine.
