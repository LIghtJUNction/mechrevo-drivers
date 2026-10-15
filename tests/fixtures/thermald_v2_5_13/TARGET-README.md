# Native target regression snapshots

`target_thd_zone.cpp`, `target_native_order.cpp` and
`target_xml_existing_zone.cpp` retain Intel's GPL
copyright/license headers and exact production code from thermald v2.5.13,
commit `5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a`:
<https://github.com/intel/thermal_daemon/tree/v2.5.13/src>.
`target_provenance.json` pins full original files, selected snapshot files,
individual functions, and the existing-zone XML branch by SHA-256.
The existing `COPYING` supplies the GPL license.

The executable runs the original existing-zone sensor check and valid-sensor
positive-temperature trip construction/binding branch, `cthd_zone::add_trip`,
`cthd_zone::update_trip_temp`, and the previously frozen real trip/cooling/RAPL
functions. The branch slice ends immediately before the zero-temperature else;
that unrelated association path is excluded. Optional
`THERMALD_PINNED_SOURCE_DIR` checks whole original sources and exact slice bytes.

Input originates from this repository's `PowerController.xml(55000000,
"ENERGY_CONSERVE", 90)`. ElementTree maps those XML values to typed parser
structures. That mapper doubles libxml parsing and does not validate the real
parser or entire daemon startup. Engine discovery, sensors, sysfs, non-RAPL
devices and polling-trip maintenance are in-memory services. Dependency, PID,
IRQ and shutdown paths abort if entered or are excluded; no hardware is opened.

Tests check same-type PASSIVE replacement without losing ACTIVE, native sensor
identity, all five ordered zero-influence bindings, 90-degree activation,
sequential RAPL-to-next-device escalation, missing-device and invalid-sensor
fallback, and explicit negative examples for old95 or missing/reordered bindings.
These are source-level behavior regressions, not physical thermal acceptance.
