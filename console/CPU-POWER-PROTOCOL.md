# CPU PL1 protocol v1 (implementation contract)

This is independent of OS PowerProfile and OEM EC control. Reuse the existing
qualified Python PowerController; no new RAPL loop or EC address writes.

Linux installs a dedicated root-owned executable at
`/usr/lib/mechrevo-power-control/console-helper`. Its shebang uses the fixed
system Python with `-I`; it loads only the fixed installed controller module.
No source-directory fallback, PATH/PYTHONPATH imports, user paths or root option.
Polkit action `org.mechrevo.console.cpu-power` fixes this path and argv1
`--request`. The helper accepts exactly `--request JSON`, at most 32 KiB UTF-8.
The power-control package owns helper and policy; the Rust console can report
their absence. Existing root CLI, state v1 and thermald ExecStartPost stay valid.

Requests reject unknown fields, duplicate keys, invalid versions/nonces and
non-integral/bool numeric values before touching the controller. Request IDs are
1..128 ASCII alphanumeric or hyphen. All object keys below are required,
including explicit null policy options.

```json
{"schema_version":1,"request_id":"example-1","operation":{"kind":"probe"}}
```

```json
{"schema_version":1,"request_id":"example-2","operation":{"kind":"execute","plan":{"revision":"64-lowercase-hex","action":{"kind":"set","policy":{"cap_uw":55000000,"target_celsius":90,"polling_seconds":1,"rapl_debounce_seconds":0}}}}}
```

Action kinds are `set` (complete policy required), `clear`, `recover`, `reapply`
(the latter three have no additional keys). cap_uw is positive whole watts,
within the controller's actual permitted operational interval. Options are
explicit null or the currently qualified 90/1/0 combination, with the existing
dependency checks. No implicit defaults may erase an existing option.

Reply keys: schema_version, request_id, outcome, status, receipt, error.
outcome is `observed`, `applied_verified`, `rejected_before_write`,
`failed_restored_verified` or `failed_state_unknown`. status/receipt/error may be
null. error uses ControlError {code,message,details:[string]}; retain primary,
rollback, receipt-write and post-read errors separately. Never classify an
outcome by matching English error messages. receipt is bounded original JSON,
never an input for choosing paths or writes. A successful executed action needs
actual transaction evidence and corresponding qualified post-state, not merely
exit zero. Probe can retain partial status together with an explicit error.

Status keys (all required):

| Key | Value |
| --- | --- |
| management | `native`, `managed`, `recovery_pending`, `unknown` |
| revision | 64 lowercase hex or null |
| qualified | bool |
| allowed_actions | array of `set`, `clear`, `recover`, `reapply` |
| current_policy | complete policy object or null |
| operational_ceiling_uw | positive integer or null; recorded operational bound, not recommended TDP |
| minimum_limit_uw | 35000000 |
| thermal_spec_reference_uw | 55000000; reference, not the initial operational ceiling |
| observed | array of {interface: `msr` or `mmio`, pl1_uw: positive integer or null} |
| thermald_active | bool or null |
| holding_now | bool; only true with qualified complete managed state |
| blocking_errors | array of strings |
| diagnostic_errors | array of strings; optional diagnostics do not become write qualification |

Probe is explicitly authorized and read-only except for its coordination lock.
It must qualify unmanaged hosts too. State-read errors are unknown, never a
confirmed native policy. A pending transaction may expose only recover after
the existing ownership/recovery prerequisites are verified. No guessed range.

The revision binds complete persistent state bytes, managed files, preference,
qualification/host/boot and interface identities. Exclude live temperature,
optional diagnostics and managed PL1 fluctuations; native initial operating
limits must bind the initial cap boundary. Recompute and compare inside each
existing operation's initial lock, before backup/state/config/actuator writes.
Do not hold an outer lock across restart: ExecStartPost apply needs that lock.
Keep current conditional rollback, foreign-state and pending-owner protection.

Rust Client construction, startup, snapshot and automatic sampling never
authorize or call this helper. Explicit probe supplies a cached observation;
local preview validates its typed action and preserves the complete policy.
Only explicit execution invokes pkexec. Validate fixed entry path ownership,
request correlation, schema, action/outcome/status and real exit consistency.
Drain bounded output and wait for the actual process; preserve raw bounded
stdout/stderr on errors. Never kill or timeout a potentially privileged writer.

Client returns CpuPowerUpdate {status, outcome, error, receipt, helper_reaped}.
Distinguish reaped with unknown state from unknown process lifetime. Windows
returns explicit unsupported. No fallback to OS profile or raw RAPL writes.

GUI uses the existing Controls page with a local CPU section. Probe is a clear
explicit authorization button; full policy is edited, previewed and applied.
Show MSR/MMIO readings, management, observed time, operational bound and thermal
reference separately. Native/90, native/1, native/0 are distinct choices.
Clear/recover explain that they return to native thermald policy, not a fixed
watt value. Never silently discard another control's pending preview. Fan
lease/recovery gating remains. Partial errors remain visible through unrelated
refreshes. State unknown blocks ordinary writes until a fresh successful probe;
liveness unknown additionally blocks close. Closing never clears a persistent
CPU policy and waits for any actual in-flight helper.

Verification must exercise real controller and production Rust/GUI boundaries,
including stale revisions under the existing lock, complete-option retention,
downward-only reapply, foreign changes, pending owner, rollback/receipt failures,
authorization failure, malformed/partial output, and writer lifecycle. Offline
tests and compile checks do not prove hardware effect or Windows compatibility.
