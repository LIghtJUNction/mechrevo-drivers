# Clevo ioctl error handling

`test_clevo_ioctl_runtime.py` compiles the actual `clevo_ioctl_interface` and
`fop_ioctl` C bodies, with the real ioctl header, Clevo firmware command constants
and fan limits. Kernel copying, firmware calls, DMI lookup and sleeping are safe
userspace spies. It runs both normal and `DEBUG` builds without root or hardware.

The source-confirmed defects are discarded user-copy results, discarded firmware
errors, output copying after failed reads, webcam SET after a failed prerequisite
GET, and the manual-fan delay following a failed SET. The fix returns `-EFAULT`
for any incomplete user copy, returns existing signed transport errors unchanged,
and stops dependent operations after a nonzero transport result.

The actual integer transport path is `clevo_evaluate_method` to the active WMI or
ACPI interface. These producers return zero or negative values, including the
existing `-1` result for a missing firmware reply and `-ENODATA` for a non-integer
reply. They do not return raw positive `acpi_status` values. The ioctl keeps a
signed local status and preserves a defensive unexpected nonzero value verbatim.
A positive ioctl return is a userspace success value, not an errno; the fixture
only checks that this value is preserved and dependent operations stop. This
patch does not redefine transport status conventions or change those producers.

The tests cover all seven public Clevo reads and six writes, partial and full
copy faults, original negative errors, full 32-bit read/write payloads, all 256
fan speeds in each of the three packed byte positions, the unchanged integer
thresholds and ignored fourth fan byte, and all 256 performance-profile low
bytes. They also check the webcam low-bit no-op workaround, prerequisite GET and
SET failures, existing DMI exclusions (including the asymmetric Gen4 read/write
policy), the 100 ms delay only after a successful manual SET, and the existing
zero return for unknown or unused debug commands.

All test commands enter through the actual `fop_ioctl`, with only its Uniwill
handler stubbed. The existing copy and transport cases verify that Clevo errors
remain negative through that function and stop dispatch; the success and unknown
command cases check payloads and continued routing. Changing the outer
function's local `status` from `long` to `u32` must fail these tests even when the
inner Clevo function is already fixed.

Run:

```sh
PYTHONDONTWRITEBYTECODE=1 python -m unittest discover -s tests -p test_clevo_ioctl_runtime.py -v
```

For a pre-fix sensitivity run, point `CLEVO_IO_SOURCE_ROOT` at an unchanged source
tree. The same fixture and public-header definitions are used. Assertions must
fail for the actual lost-error paths; compilation failure does not count as a
successful sensitivity check.

These tests and kernel-module compilation do not validate Clevo firmware or
hardware behavior. The available GM6PX0X laptop uses Uniwill; no Clevo physical
device has been tested, and no Clevo firmware settings are changed by these tests.
