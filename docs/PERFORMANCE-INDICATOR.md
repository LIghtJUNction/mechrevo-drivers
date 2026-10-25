# GM6PX0X performance button and indicator investigation

Scope: MECHREVO Kuangshi16Pro Series GM6PX0X, board GM6PX0X,
BIOS N.1.09MRO07. Other models and firmware are unqualified.

## Confirmed input and profile defects

The Uniwill mode event 0xB0 generates Meta + Alt + F6 on the TUXEDO input
device. Combining a buffered Python input stream with select() can leave F6
in the userspace buffer after the file descriptor stops reporting readiness.
A real pipe carrying the complete six-event chord reproduces the problem.
The listener now reads without buffering and reports disconnects/short events.

TCC's SetTempProfileById acknowledges a queued request. Its true return value
does not prove the selected profile is active. The control tool checks the
boolean reply and waits for the requested active-profile ID before requesting
the indicator or announcing completion. This is profile identity confirmation,
not proof of CPU power limits, thermal performance, or physical illumination.

## OEM evidence from leanrea analysis

The preserved OEM 5.17.51.34 service is a protected .NET PE. Analysis uses the
previously restored CIL bodies rather than the exception stubs produced by an
ordinary decompiler. No OEM code was executed.

Original service SHA256:
`a2b7d81ecf5c1dbdfc19738f17ba5e01e884d1b3688beb668858727307e2a318`.

For the observed project 0x10 and RAM-fan-1p5 capability, the selected manager
is MyFanManager_RamFan1p5. The mode-button path is:

`WMIHandleEvent 06001220` (0xB0) → `MyFanCtrl.ModeSwitchChanged 06000ed3` →
`RamFan1p5.ModeSwitchChanged 060016d9` → worker → `SetUserProfile 06001700` →
`SetFanMode 06001705`.

SetFanMode body SHA256:
`528db385738f28f747c19302463802dcac8a57514cc45140bab3d341e1395ab3`.

| Selected OEM mode | 0x0751 without FanBoost | 0x0727 bit 6 | 0x0726 bit 7 |
| --- | --- | --- | --- |
| Office (0) | 0xa0 | Clear | Clear |
| Gaming (1) | 0x00 | Clear | Clear |
| Turbo (2) | 0x10 | Clear | Clear |
| Custom (3) | 0x00; 0x10 with overclock enabled | Set | Set |

FanBoost adds bit 6 in 0x0751. The 0x0727 and 0x0726 operations preserve the
other bits. CustomerModeLightOn/Off are real calls on this selected manager;
the TUXEDO model whitelist alone cannot disprove their OEM applicability.
The physical meaning on this machine still needs observation.

The ACPI table also reads 0x0751 bit 4 (TBME) and bit 7 (UFME) in GPU/power
policy. Consequently W_UW_MODE is a hardware-mode operation, not an independent
LED-brightness interface. Its readback cannot establish emitted light.

## Rejected 0x07a5 candidate

SetPowerLedStatus (06001715) preserves the high six bits of 0x07a5 and changes
the low two. Similar Intel/QC methods take the system-power enum, not the
Office/Gaming/Turbo enum. No direct call to this private method was found in
the selected manager's restored path. This is a candidate implementation,
not evidence that the button uses it.

On 2026-10-26 a temporary, exact-host-gated module tested low-field value 2.
Both WMI and the OEM INOU.ECRR path read back 2, but the user reported **no
visible change**. The original low-field value 0 was restored and the temporary
module unloaded. It is not shipped or installed as a supported lamp feature.
Do not turn that successful register write into a claim of repaired lighting.

Private recovery and trial receipts are under
`/home/lightjunction/Backups/mechrevo-drivers/2026-10-26-mode-led/`.
Reproducible local disassembly and the rejected source remain in the ignored
`.local-tests/mode-led-live-20261026/` directory. These are not runtime dependencies.

## Remaining acceptance

The three physical lamp states, mode transitions, and wake/resume behavior are
not yet established. The existing W_UW_MODE bridge does not prove an independent
lamp interface. Further changes require the actual OEM branch plus physical
confirmation; the failed candidate must not be reintroduced as a silent fallback.
