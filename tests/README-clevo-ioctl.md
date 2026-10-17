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
policy), and the 100 ms delay only after a successful manual SET.

Unknown commands and commands with an incorrect direction, size or type return
`-ENOTTY` before copying user data or calling firmware. Each handler matches the
complete command value. The outer entry selects one handler and returns its
signed result directly; the three generic reads do not enter either handler.
The two historical `DEBUG` commands, `R_TF_BC` and `W_TF_BC`, keep their Clevo
magic values but route to Uniwill. The read alias retains its original `_IOW`
encoding. Ordinary builds reject both aliases. Uniwill's `W_UW_MODE_ENABLE`
remains a successful no-op, and `W_UW_FANAUTO` still accepts no argument.

All test commands enter through the actual `fop_ioctl`, with only its Uniwill
handler stubbed. The existing copy and transport cases verify that Clevo errors
remain negative through that function. Successful Clevo commands never enter the
Uniwill handler. The Clevo and Uniwill suites cover malformed encodings of all
45 ordinary public commands, plus the two historical debug aliases. The public
header is unchanged; these tests do not establish 32-bit userspace compatibility
with a 64-bit kernel, for which the driver has no `compat_ioctl` entry.

Run:

```sh
PYTHONDONTWRITEBYTECODE=1 python -m unittest discover -s tests -p test_clevo_ioctl_runtime.py -v
```

The complete affected domain runs through the repository verification runner as
an ordinary user:

```sh
mkdir -p .local-tests
python3 -B scripts/verify.py --domain driver
```

For a pre-fix sensitivity run, point `CLEVO_IO_SOURCE_ROOT` at an unchanged source
tree. The same fixture and public-header definitions are used. Assertions must
fail for the actual lost-error paths; compilation failure does not count as a
successful sensitivity check.

These tests and kernel-module compilation do not validate Clevo firmware or
hardware behavior. The available GM6PX0X laptop uses Uniwill; no Clevo physical
device has been tested, and no Clevo firmware settings are changed by these tests.
