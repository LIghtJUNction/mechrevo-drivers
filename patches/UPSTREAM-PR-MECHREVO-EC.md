# Linux hwmon upstream patch: MECHREVO GM6PX0X

**Status:** prepared and locally verified; not sent to the kernel mailing list.

Patch: [`0001-hwmon-mechrevo-ec-v1.patch`](0001-hwmon-mechrevo-ec-v1.patch)

The patch adds a read-only hwmon driver for the verified `Kuangshi16Pro Series GM6PX0X`, board `GM6PX0X`, BIOS `N.1.09MRO07`. It reads CPU/GPU EC temperatures and fan RPM using `INOU.ECRR`, checks the EC project and fan capability, and makes no EC writes. Other models and BIOS versions are not included.

## Verification

- Project regression tests: 10 passed.
- Patch source: external-module `W=1` build against the running `7.2.11-arch1-1` kernel headers passed.
- Hardware: temporary patch module loaded beside the installed driver, exposed all four read-only attributes, and unloaded while the installed hwmon device remained. Five sequential comparison pairs tracked the same sensor ranges; temperatures and fan speeds changed between reads.
- The first patch draft had an ACPI integer-union initializer error and was rejected by the firmware. That draft emitted ACPI parameter warnings. The initializer was fixed, the module rebuilt, and the corrected module passed hardware reads. The historical warnings remain in the kernel log.
- `checkpatch.pl --strict --no-signoff`: 0 errors, 1 advisory asking whether `MAINTAINERS` needs an entry. The existing general HWMON entry already covers `drivers/hwmon/` and `Documentation/hwmon/`; no individual maintainer commitment has been added.

This is not a full kernel-tree build, installation, boot-time loading test, upstream submission, or validation on another laptop/BIOS. The patch intentionally has no `Signed-off-by` line. Kernel contribution guidance requires the human submitter to review the code and add their own DCO sign-off; an AI agent must not sign on their behalf.
