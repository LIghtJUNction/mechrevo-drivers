# GM6PX0X fan-2 ECRR offset comparison (read-only)

This is a temporary diagnostic for Armin Wolf's 2026-10-30 question:
does \`0x046D\`, as used by \`uniwill_laptop\`, provide a valid second-fan
low byte on the already-tested MECHREVO GM6PX0X / BIOS N.1.09MRO07?
It does not change the mainline hwmon patch or select a winning register.

## What it measures

The only firmware method invoked by the small, independently buildable
kernel module is \`\\_SB.INOU.ECRR(address)\`. Probe reads \`0x0740\`
and \`0x078E\` to qualify the project and fan capability. Each sample reads:

\`\`\`text
H0 = ECRR(0x046C)
B  = ECRR(0x046B)  /  D = ECRR(0x046D)
D  = ECRR(0x046D)  /  B = ECRR(0x046B)
H1 = ECRR(0x046C)
\`\`\`

Only one of the two middle orders is used per sample; the module alternates
them on subsequent reads. It waits 6–12 ms after each successful ECRR call,
matching the original hwmon patch and the Uniwill transport. When H0 differs
from H1, the sample is not decoded and is retried, up to three attempts. The
raw attempts, including rejected high-byte turnovers, are returned by the
root-only \`/sys/module/gm6px0x_fan2_ecrr/sample\` attribute.

For a stable attempt only, the userspace collector computes:

\`\`\`text
candidate_B = (H0 << 8) | B
candidate_D = (H0 << 8) | D
\`\`\`

Both are **candidate raw counts**, not externally calibrated RPM.
High/low/high cannot detect every low-byte refresh; the 6–12 ms spacing
also prevents a truly atomic snapshot. Alternating the middle-byte order and
comparing multiple samples reduces, but does not eliminate, timing bias.

## Safety boundary

- Requires exact DMI vendor, product, **board**, and BIOS, plus project
  \`0x10\` and capability \`0x078E & 0x40\`; mismatch fails before sampling.
- No module parameter, arbitrary address interface, \`ECRW\`, \`regmap_write\`,
  keyboard/CTGP/lightbar adjustment, background service, or DKMS installation.
- Never load \`uniwill_laptop\` with \`force=1\`. Never modify fan mode or
  generate artificial load for this experiment.
- Calling \`ECRR\` is a firmware method invocation. The recorded DSDT
  describes a memory-mapped byte read at \`0xFE410000 + address\`, and the
  module issues no EC write method. That evidence is stronger than a
  source-level no-write claim, but does not audit every EC firmware side effect.
- The module is temporary. Loading a kernel module requires administrative
  authority and correct running-kernel headers. Review source and build before
  an **explicit** load on the qualified host; no command below auto-loads
  the module except the clearly marked \`insmod\`.

## Reproduce only on the qualified laptop

From the repository root:

\`\`\`bash
uname -r
for field in sys_vendor product_name board_name bios_version; do
  printf '%s: ' "$field"
  cat "/sys/class/dmi/id/$field"
done

mkdir -p .local-tests
test_dir="$(mktemp -d -p .local-tests gm6px0x-fan2-XXXXXX)"
cp scripts/gm6px0x_fan2_ecrr/Makefile \
   scripts/gm6px0x_fan2_ecrr/gm6px0x_fan2_ecrr.c "$test_dir/"
make -C "/usr/lib/modules/$(uname -r)/build" M="$PWD/$test_dir" modules -j2
\`\`\`

Only after inspecting the module and confirming the exact DMI / BIOS:

\`\`\`bash
(
  set -e
  sudo insmod "$test_dir/gm6px0x_fan2_ecrr.ko"
  trap 'sudo rmmod gm6px0x_fan2_ecrr' EXIT
  sudo python3 scripts/gm6px0x_fan2_ecrr/collect.py \
    --live --samples 30 --interval 1 \
    --output "$test_dir/fan2-ecrr.jsonl"
)
\`\`\`

This records at most 30 bounded snapshots over roughly 30 seconds at
normal background fan behavior. An exception stops sampling, records an
error row, and the shell trap unloads the temporary module. Re-running with
the same output filename refuses to overwrite it. Keep the \`.jsonl\` file;
each row is flushed even when a later call fails. An interrupted process may
lack the final summary but already captured rows can be examined offline.

No root access is required to analyze an existing capture:

\`\`\`bash
python3 scripts/gm6px0x_fan2_ecrr/collect.py \
  --analyze "$test_dir/fan2-ecrr.jsonl"
python3 -m unittest discover -s tests -p 'test_gm6px0x_fan2_ecrr.py' -v
\`\`\`

When sharing results, include the \`jsonl\`, program revision, kernel
version, exact model/board/BIOS, and whether the fan naturally varied.
Do not include serial numbers, UUIDs, MAC addresses, or entire DMI dumps.

## How to interpret the data

| Finding | Minimum evidence | Meaning |
| --- | --- | --- |
| Rejected turnover | \`hi0 != hi1\` | Neither candidate's 16-bit value from that attempt is used |
| Invalid byte / ACPI error | Range violation or ECRR failure | Row fails closed; never substitute zero |
| \`0x00\` / \`0xFF\` | Repeated sentinel-looking value | Flag for inspection; one raw byte alone is not proof of invalidity |
| Similar readings | At least 20 stable samples, enough natural variation, at least 95% within a diagnostic 1% band, strong tracking | Supports equivalence **over the observed range** only |
| Disagreement | Repeated stable, changing B readings while D is flat or notably divergent | Evidence against directly substituting D for B on this BIOS |
| Constant fan / too few samples | Insufficient variation | Inconclusive even if every number matches |

The 1% band and 20-sample gate are *heuristics*, not hardware-spec limits.
The code always sets \`independent_physical_rpm_validation=false\` and
\`driver_change_justified=false\`; these require additional review.
Do not change the posted \`0x046C/0x046B\` patch merely because
\`0x046D\` exists in another driver. If the two candidates track, report that
fact to the reviewer with raw rows; if they diverge, report the counterexample.
No static evidence alone establishes which byte measures mechanical RPM.
