# Pull Request: Handle SBIOS OEM 0xC5 notification without triggering NV_ASSERT in rm_acpi_notify

**Target Repository**: `https://github.com/NVIDIA/open-gpu-kernel-modules`
**Branch**: `main`

## Description

On multiple Uniwill/Mechrevo platforms (such as TongFang GM6PX0X and related chassis running 12th/13th Gen Intel + RTX 40-series mobile GPUs), the platform ACPI firmware emits two companion notifications whenever the AC power state or platform performance mode changes.

In ACPI DSDT:
```asl
Method (PWBT, 1, NotSerialized)
{
    If (((^^PC00.LPCB.EC0.TBME == One) && (^^PC00.LPCB.EC0.UFME == Zero)))
    {
        ...
        Notify (NPCF, 0xC0)              // Dynamic Boost / cTGP notification
        Notify (^^PC00.PEG1.PEGP, 0xC5)  // Sent directly to dGPU ACPI device
    }
}
```

The second notification (`0xC5` / decimal `197`) is sent to the PEG device node, which is hooked by `nv_acpi_register_notifier()`:
```c
nv_acpi_notify_event(acpi_handle handle, u32 event_type, void *data) ->
    rm_acpi_notify(pNvAcpiObject->sp, NV_STATE_PTR(nvl), event_type);
```

In `src/nvidia/arch/nvalloc/unix/src/osapi.c:2163`:
```c
void NV_API_CALL rm_acpi_notify(
    nvidia_stack_t *sp,
    nv_state_t *nv,
    NvU32 event_type
)
{
    ...
    switch (event_type)
    {
        case ACPI_VIDEO_NOTIFY_PROBE: ...
        case ACPI_NOTIFY_GPS_STATUS_CHANGE: ...
        case ACPI_NOTIFY_POWER_LEVEL_D1: ...
        default:
            NV_PRINTF(LEVEL_INFO, "No support for 0x%x event\n", event_type);
            NV_ASSERT(0);
            break;
    }
}
```

Because `0xC5` is not listed, execution falls through to `default:`, emitting an assertion failure into the Linux kernel log on every boot or power profile transition:
```text
NVRM: nvAssertFailedNoLog: Assertion failed: 0 @ osapi.c:2198
```

## Solution

1. Define `ACPI_NOTIFY_OEM_POWER_BUDGET (0xC5)` in `acpi_common.h`.
2. Add an explicit case in `rm_acpi_notify()` to acknowledge the notification.
3. Remove `NV_ASSERT(0)` from the `default:` branch of `rm_acpi_notify()`. Per ACPI specification section 5.6.6, device-specific notification values (`0x80 - 0xFF`) may be utilized by OEM platform firmware. The driver should log unsupported vendor notifications at info/debug level rather than firing assertion failures in production/debug builds.

## Verification

- Tested on Linux 7.2.9-arch1-1 with NVIDIA 615.78.08 (RTX 4060 Mobile).
- Confirmed that boot and profile change events no longer trigger assertion failures in `dmesg`.
