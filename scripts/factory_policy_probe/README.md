# GM6PX0X MRO07 factory policy reader

This temporary, root-only probe exposes the **raw selected factory buffer**.
It does not set EC power, change fan curves, or replace the installed driver.
Only MECHREVO `Kuangshi16Pro Series GM6PX0X` / BIOS `N.1.09MRO07` is accepted.

The source follows this host's DSDT `SMRW` selection branches using `PPFG` and
the GPU module byte `GFID` at EC 0x07d2. It reads the selected named ACPI buffer,
then compares three DWORDs returned by fixed **0xbb read commands** at offsets
0, 4, and 8. Request buffers are 13 bytes, covering the fields declared by this
firmware. No 0xaa command, arbitrary method/address, or writable attribute is
provided. Unsupported identity branches skip SMRW. ACPI errors, malformed
buffers, out-of-range values, and comparison mismatches fail the snapshot.

Build in a separate directory with matching running-kernel headers:

```sh
mkdir -p .local-tests/factory-policy
cp scripts/factory_policy_probe/Makefile scripts/factory_policy_probe/mechrevo_policy_probe.c .local-tests/factory-policy/
make -C /usr/lib/modules/"$(uname -r)"/build M="$PWD/.local-tests/factory-policy" modules -j2
(
  set -e
  sudo insmod .local-tests/factory-policy/mechrevo_policy_probe.ko
  trap 'sudo rmmod mechrevo_policy_probe' EXIT
  sudo cat /sys/module/mechrevo_policy_probe/snapshot
)
```

The tested host returned phase 4, GFID 0x11, AX44, and raw bytes
`8282c8050019577dffffffff`. All three read-command results reproduced those
bytes. The nine-field Windows metadata struct suggests possible field names;
its consumer is still protected. **The byte units, encoding, permitted ranges,
and complete activation/recovery sequence are not established.** These bytes
must not be installed as CPU/GPU limits or interpreted as active RAPL settings.

This is a research reader, not an installed feature or a performance preset.
The final source was built and read on kernel 7.2.9-arch1-1, root-only access was
checked, and the module was unloaded. Other hardware and BIOS versions are
unverified. No persistent module loading, DKMS registration, or service is added.

Regression tests execute the actual C source with ACPI/sysfs test doubles:

```sh
python -m unittest discover -s tests -p test_factory_policy_probe.py -v
```

They cover all nine selection branches, unsupported identities, fixed read
packets, object/length/range errors, read failures, mismatched DWORDs, lock
release, DMI/method gates, and attribute-creation failure. Mock errors do not
constitute real firmware fault injection.
