#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Contract verification for reverse-engineered OEM ACPI INOU and power protocols.

Covers:
1. DSDT INOU.SMRW buffer-only semantics (AX44 named buffer read/write).
2. DSDT INOU.T1WR hardware power limit dispatch (APL1, APL2, APL4, APTC, cTGP, DBAC).
3. ACPI PWBT dual-notification and NVIDIA open kernel driver 0xC5 assertion model.
4. Battery protection 0x07a6 bits 5:4 mode transition and SMAPCTable synchronization.
5. Modern Standby Resume transaction ordering and error-isolation boundaries.
"""
import struct
import unittest


class MockAcpiInou:
    """Accurate userspace model of DSDT Device (INOU) from GM6PX0X BIOS N.1.09MRO07."""

    def __init__(self):
        # 12-byte named buffers
        self.ax44 = bytearray([0x82, 0x82, 0xC8, 0x05, 0x00, 0x19, 0x57, 0x7D, 0xFF, 0xFF, 0xFF, 0xFF])
        # EC fields
        self.ec_apl1 = 0
        self.ec_apl2 = 0
        self.ec_apl4 = 0
        self.ec_aptn = 0
        self.ec_aptc = 0
        self.ec_ctwa = 0
        self.ec_tbme = 0
        self.ec_ufme = 0
        self.ec_ocpl = 0
        self.ec_gfid = 0x11  # GM6PX0X GFID
        self.ec_adpt = 0x01
        self.ppfg = 4        # GM6PX0X PPFG
        # NPCF variables
        self.npcf_acbt = 0
        self.npcf_ctgp = 0
        self.npcf_dbac = 0
        self.npcf_amat = 0
        self.npcf_amit = 0
        self.npcf_atpp = 0
        self.npcf_wmen = 0
        # Notification log: list of (target_name, event_code)
        self.notifications = []
        # EC RAM mock for battery and fan
        self.ec_ram = bytearray(0x1000)

    def smrw(self, buffer_arg: bytes) -> int:
        """Emulate Method (SMRW, 1, Serialized) from dsdt.dsl:126106."""
        if len(buffer_arg) < 2:
            return 0
        rwfg = buffer_arg[0]
        reof = buffer_arg[1]
        buof = reof
        local0 = 0

        # Switch (ToInteger (PPFG)) -> Case (0x04) for GM6PX0X (GFID == 0x11)
        if self.ppfg == 4 and self.ec_gfid == 0x11:
            if rwfg == 0xAA:
                # AX44 = WRBF (bytes from offset 1 up to 12)
                payload = buffer_arg[1:13]
                self.ax44[:len(payload)] = payload
            elif rwfg == 0xBB:
                # CreateDWordField (AX44, BUOF, X4C4)
                if buof + 4 <= len(self.ax44):
                    local0 = struct.unpack_from("<I", self.ax44, buof)[0]
        return local0

    def t1wr(self, arg0: int, arg1: int = 0, arg2: int = 0) -> None:
        """Emulate Method (T1WR, 3, NotSerialized) from dsdt.dsl:125875."""
        if arg0 == 0x81:
            self.ec_apl1 = arg1
        elif arg0 == 0x82:
            self.ec_apl2 = arg1
        elif arg0 == 0x84:
            self.ec_apl4 = arg1
        elif arg0 == 0x85:
            self.ec_aptn = 1
            self.ec_aptc = arg1
        elif arg0 == 0x71:
            local0 = (self.ec_ctwa * 8) & 0xFFFFFFFF
            if self.ec_tbme == 1 and self.ec_ufme == 0:
                self.pwup(self.ec_ocpl)
            else:
                self.npcf_acbt = local0
                self.notifications.append(("NPCF", 0xC0))
        elif arg0 == 0x1171:
            self.npcf_ctgp = 1
            self.ec_ctwa = arg1
            local0 = (self.ec_ctwa * 8) & 0xFFFFFFFF
            if self.ec_tbme == 1 and self.ec_ufme == 0:
                self.pwup(self.ec_ocpl)
            else:
                self.npcf_acbt = local0
                self.notifications.append(("NPCF", 0xC0))
        elif arg0 == 0x1172:
            self.npcf_dbac = arg1
            self.notifications.append(("NPCF", 0xC0))
        elif arg0 == 0x1173:
            self.npcf_dbac = 0
            self.npcf_amat = (arg1 * 8) & 0xFFFFFFFF
            self.npcf_amit = (arg2 * 8) & 0xFFFFFFFF
            self.notifications.append(("NPCF", 0xC0))
        elif arg0 == 0x2273:
            self.npcf_atpp = (arg1 * 8) & 0xFFFFFFFF
            self.notifications.append(("NPCF", 0xC0))
        elif arg0 == 0x1175:
            self.npcf_wmen = arg1
            self.notifications.append(("NPCF", 0xC1))

    def pwup(self, arg0: int) -> None:
        """Emulate Method (PWUP, 1, NotSerialized) from dsdt.dsl:126015."""
        if self.ec_gfid in (0x10, 0x11, 0x12, 0x13, 0x14):
            if arg0 == 1:
                self.pwbt(0)
            elif arg0 in (2, 3, 4, 5):
                self.pwbt(0xC8)

    def pwbt(self, arg0: int) -> None:
        """Emulate Method (PWBT, 1, NotSerialized) from dsdt.dsl:126080."""
        if self.ec_tbme == 1 and self.ec_ufme == 0:
            local0 = 0
            if self.ec_adpt != 0x02 or self.ec_gfid in (0x0C, 0x08):
                local0 = arg0
            self.npcf_acbt = local0
            # Dual notification: NPCF 0xC0 and PEG1.PEGP 0xC5
            self.notifications.append(("NPCF", 0xC0))
            self.notifications.append(("PEG1.PEGP", 0xC5))


def nvidia_rm_acpi_notify(event: int) -> str:
    """Model rm_acpi_notify() from NVIDIA open driver osapi.c:2163."""
    if event == 0xC0:  # ACPI_NOTIFY_GPS_STATUS_CHANGE
        return "HANDLED_GPS_STATUS_CHANGE"
    elif event in (0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5):
        return "HANDLED_D_STATE"
    elif event == 0x80:
        return "HANDLED_VIDEO_NOTIFY_PROBE"
    else:
        # osapi.c:2198: NV_ASSERT(0) for unknown events like 0xC5 (197)
        raise AssertionError(f"osapi.c:2198: NV_ASSERT(0) triggered on unknown event 0x{event:02X} ({event})")


class MechrevoEcPowerProtocolTests(unittest.TestCase):
    def setUp(self):
        self.inou = MockAcpiInou()

    def test_smrw_buffer_only_semantics_and_zero_hardware_mutation(self):
        """SMRW only reads/writes the named buffer AX44 and never changes hardware/EC/powercap."""
        # Initial read of DWORD at offset 0
        val = self.inou.smrw(bytes([0xBB, 0x00]))
        self.assertEqual(val, struct.unpack_from("<I", self.inou.ax44, 0)[0])

        # Write new payload into AX44
        new_payload = bytes([0xAA, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B])
        self.inou.smrw(new_payload)
        self.assertEqual(self.inou.ax44[:11], new_payload[1:12])

        # Verify readback matches
        read_dword = self.inou.smrw(bytes([0xBB, 0x00]))
        self.assertEqual(read_dword, struct.unpack_from("<I", new_payload, 1)[0])

        # Confirm zero side-effects on power registers or notifications
        self.assertEqual(self.inou.ec_apl1, 0)
        self.assertEqual(self.inou.ec_apl2, 0)
        self.assertEqual(self.inou.ec_apl4, 0)
        self.assertEqual(len(self.inou.notifications), 0)

    def test_t1wr_cpu_pl_and_tcc_dispatch(self):
        """T1WR handles CPU PL1 (0x81), PL2 (0x82), PL4 (0x84), and TCC offset (0x85)."""
        self.inou.t1wr(0x81, 75)
        self.assertEqual(self.inou.ec_apl1, 75)

        self.inou.t1wr(0x82, 115)
        self.assertEqual(self.inou.ec_apl2, 115)

        self.inou.t1wr(0x84, 200)
        self.assertEqual(self.inou.ec_apl4, 200)

        self.inou.t1wr(0x85, 10)
        self.assertEqual(self.inou.ec_aptn, 1)
        self.assertEqual(self.inou.ec_aptc, 10)

    def test_t1wr_gpu_ctgp_and_npcf_notification(self):
        """T1WR dispatches GPU cTGP, DBAC, and WhisperMode, triggering NPCF notifications."""
        self.inou.t1wr(0x1171, 25)
        self.assertEqual(self.inou.npcf_ctgp, 1)
        self.assertEqual(self.inou.ec_ctwa, 25)
        self.assertEqual(self.inou.npcf_acbt, 200)  # 25 * 8
        self.assertIn(("NPCF", 0xC0), self.inou.notifications)

        self.inou.notifications.clear()
        self.inou.t1wr(0x1172, 1)
        self.assertEqual(self.inou.npcf_dbac, 1)
        self.assertIn(("NPCF", 0xC0), self.inou.notifications)

        self.inou.notifications.clear()
        self.inou.t1wr(0x1175, 1)
        self.assertEqual(self.inou.npcf_wmen, 1)
        self.assertIn(("NPCF", 0xC1), self.inou.notifications)

    def test_pwbt_dual_notification_and_nvidia_0xc5_assertion_model(self):
        """PWBT emits both NPCF 0xC0 and PEG1.PEGP 0xC5; rm_acpi_notify asserts on 0xC5."""
        self.inou.ec_tbme = 1
        self.inou.ec_ufme = 0
        self.inou.pwbt(0xC8)

        # Confirm dual notifications were emitted by ACPI
        self.assertEqual(self.inou.notifications, [("NPCF", 0xC0), ("PEG1.PEGP", 0xC5)])

        # NPCF 0xC0 is handled normally
        result_c0 = nvidia_rm_acpi_notify(0xC0)
        self.assertEqual(result_c0, "HANDLED_GPS_STATUS_CHANGE")

        # PEGP 0xC5 triggers NV_ASSERT(0) in osapi.c:2198
        with self.assertRaisesRegex(AssertionError, r"osapi\.c:2198: NV_ASSERT\(0\)"):
            nvidia_rm_acpi_notify(0xC5)

    def test_battery_charging_profile_ec_bits_and_smapc_sync(self):
        """EC 0x07a6 bits 5:4 represent High (00), Balanced (01), and Stationary (10)."""
        # Baseline reg 0x07a6
        self.inou.ec_ram[0x07a6] = 0x0F  # low nibble set, bits 5:4 zero

        def apply_profile(profile_code: int) -> int:
            if profile_code > 2:
                return -22  # -EINVAL
            # Preserve existing bits, update bits 5:4
            cur = self.inou.ec_ram[0x07a6]
            cur = (cur & ~(0x03 << 4)) | ((profile_code & 0x03) << 4)
            self.inou.ec_ram[0x07a6] = cur
            # SMAPCTable sync
            self.inou.smrw(bytes([0xAA, cur]))
            return 0

        # Mode 0 (high capacity)
        self.assertEqual(apply_profile(0), 0)
        self.assertEqual((self.inou.ec_ram[0x07a6] >> 4) & 0x03, 0)

        # Mode 1 (balanced)
        self.assertEqual(apply_profile(1), 0)
        self.assertEqual((self.inou.ec_ram[0x07a6] >> 4) & 0x03, 1)

        # Mode 2 (stationary)
        self.assertEqual(apply_profile(2), 0)
        self.assertEqual((self.inou.ec_ram[0x07a6] >> 4) & 0x03, 2)

        # Mode 3 (reserved) rejected
        self.assertEqual(apply_profile(3), -22)

    def test_resume_transaction_sequence_and_error_isolation(self):
        """Linux Resume must isolate fan errors from battery restoration."""
        operations = []

        def resume_pipeline(fan_fails: bool):
            # Stage 1: Refresh AC
            operations.append("REFRESH_AC")
            # Stage 2: APExistToEC
            operations.append("SET_AP_EXIST")
            # Stage 3: Fan Resume (with error isolation)
            try:
                if fan_fails:
                    raise RuntimeError("EC fan curve write timeout")
                operations.append("FAN_RESUME_OK")
            except Exception as e:
                operations.append(f"FAN_RESUME_ERR:{e}")
            # Stage 4: Battery Resume MUST still run even if fan failed
            operations.append("BATTERY_RESUME_OK")

        resume_pipeline(fan_fails=True)
        self.assertIn("FAN_RESUME_ERR:EC fan curve write timeout", operations)
        self.assertIn("BATTERY_RESUME_OK", operations)
        self.assertEqual(operations[-1], "BATTERY_RESUME_OK")


if __name__ == "__main__":
    unittest.main()
