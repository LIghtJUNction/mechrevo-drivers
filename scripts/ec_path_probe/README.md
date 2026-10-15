# GM6PX0X EC path comparison

This research probe compares the firmware's `\_SB.INOU.ECRR` memory reads with
the existing driver's WMI read ioctls. It does **not** implement fan control,
convert bytes to RPM, or replace the installed driver. No automatic loading,
DKMS registration, service, or persistent configuration is included.

Only MECHREVO `Kuangshi16Pro Series GM6PX0X`, BIOS `N.1.09MRO07`, is accepted.
The fixed address list comes from this host's DSDT, recovered OEM method bodies, metadata, and existing
driver reads. No arbitrary address or write interface is exposed. The snapshot
is root-readable only. A failed ACPI call or non-byte value fails the whole
snapshot instead of publishing plausible zeroes. Existing WMI reads use the
firmware mailbox internally; "read-only" describes the requested operation,
not a claim that the mailbox protocol performs no bus writes.

Build in a disposable output directory, using matching running-kernel headers:

```sh
mkdir -p .local-tests/ec-path-probe
cp scripts/ec_path_probe/Makefile scripts/ec_path_probe/mechrevo_ec_probe.c .local-tests/ec-path-probe/
make -C /usr/lib/modules/"$(uname -r)"/build M="$PWD/.local-tests/ec-path-probe" modules -j2
```

Load, sample, and always unload the temporary probe:

```sh
(
  set -e
  sudo insmod .local-tests/ec-path-probe/mechrevo_ec_probe.ko
  trap 'sudo rmmod mechrevo_ec_probe' EXIT
  sudo python scripts/compare_ec_paths.py --samples 10 --interval 1 \
    --output .local-tests/ec-path-probe/comparison.json
)
```

The output file must be new. Keep the JSON even if interrupted: completed rows
and the error are checkpointed. Sampling is bounded to 1–30 rows at intervals
of 0.5–10 seconds. The output explicitly records that GPU health was not checked
and that this diagnostic cannot qualify thermal optimization. There are no
NVIDIA calls or generated load. Do not run multiple probe instances together.

`ecrr_temp_seen_in_wmi` only says the memory value occurred in the bracketing WMI
reads; it does not establish the physical sensor, refresh rate, or fan policy.
The two paths can use different address spaces: on the tested host, ECRR reads
of 0x1804/0x1809 returned 255 while WMI returned changing fan control values.
This is why substituting one transport for the other is unsafe without further
protocol evidence. RPM-named bytes are preserved separately as raw bytes.

Verified on kernel `7.2.9-arch1-1`: build, temporary load, a 10-row investigation,
the final CLI's 3-row run, root-only snapshot access, and unload. A pre-existing
GPU Xid 79 remained outside this experiment; no GPU recovery is claimed.

Regression tests:

```sh
python -m unittest discover -s tests -p test_compare_ec_paths.py -v
```

The fixed snapshot now includes `0x078e`. The recovered OEM `IsSuportRamFan1p5`
reads its bit 6; combined with project `0x0740`, this allows a fresh transport-level
check of the native factory selection. It still does not enable manual fan control
or infer write ordering from a capability bit alone. See `docs/OEM-METHOD-BODIES.md`.
