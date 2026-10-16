---
name: mechrevo-verification
description: Verify local mechrevo-drivers changes through the repository test runner, identify missing coverage, and distinguish offline test results from installed hardware acceptance. Use for driver, power, thermal, TCC, or packaging changes in this project.
---

# Mechrevo verification

Use this skill in the mechrevo-drivers checkout. It supports verification; it does not authorize package installation, hardware writes, service changes, reboot, suspend, or executing the Windows OEM application.

## Make the invariant executable

For implementation changes, identify the actual failure and the production boundary that must reject it. For read-only reviews, identify missing checks without editing files. Prefer an existing typed state or one centralized check over repeated caller precautions. Add a regression through the actual production code or patched upstream fixture. Include success, the observed failure, and the relevant boundary; confirm the old implementation fails for the expected reason. A model or byte-pattern assertion proves only that bounded property, not hardware behavior.

## Run local verification

Use `scripts/verify.py` from this checkout as an ordinary user, under the existing AgentGC workflow. Select the affected domain, for example `python -B scripts/verify.py --domain thermal`; use its help for supported domains and configuration. Exit 0 means passed, 1 failed, and 2 incomplete. Tests may compile mock hardware harnesses and write scratch files. The runner must never install dependencies or start a hardware daemon to make a test pass.

Read the structured result, not only the command exit or unittest's `OK`. A skipped prerequisite or zero discovered tests is incomplete verification. Provide already available local tool/source inputs when the affected tests require them, then rerun only the incomplete scope. Do not substitute a lower-coverage suite for a missing prerequisite. Do not trigger or wait for remote CI for routine development.

## Hardware and delivery evidence

For a hardware behavior change, inspect the current installed package and loaded module identities independently; disk files and `modinfo` alone do not identify running code. Compare the behavior against the requirement on the qualified host using already authorized operations. If activation or physical input is unavailable, state exactly what remains untested and continue independent work.

Report separately: source revision, tests executed and skipped, installed artifact, loaded code, observed behavior, and remaining requirement. Do not infer cooling from a power limit, actuator success from a service being active, or absent activity from a trace window with no events. Before interpreting a trace, verify coverage, start/stop validity, and loss counters; missing or invalid evidence remains unknown. Keep private machine/process logs local; publish only the evidence needed to support the result.
