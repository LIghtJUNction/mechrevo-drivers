/* SPDX-License-Identifier: GPL-2.0+ WITH Linux-syscall-note */
#ifndef UNIWILL_FAN_CURVE_IOCTL_H
#define UNIWILL_FAN_CURVE_IOCTL_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define UW_FAN_CURVE_ABI_VERSION 1
#define UW_FAN_CURVE_MAGIC 0xf1
#define UW_FAN_CURVE_LEVELS 16
#define UW_FAN_CURVE_THRESHOLDS 15
#define UW_FAN_CURVE_TABLE_BYTES 96

#define UW_FAN_CURVE_IDLE 0
#define UW_FAN_CURVE_ACQUIRED 1
#define UW_FAN_CURVE_ACTIVE 2
#define UW_FAN_CURVE_RECOVERY_REQUIRED 3

#define UW_FAN_CURVE_CAP_QUALIFIED 0x01
#define UW_FAN_CURVE_CAP_FD_LEASE 0x02
#define UW_FAN_CURVE_CAP_RAW_RESTORE 0x04

#define UW_FAN_CURVE_STATE_OWNER 0x01
#define UW_FAN_CURVE_STATE_ORIGINAL_VALID 0x02
#define UW_FAN_CURVE_STATE_LIVE_VALID 0x04
#define UW_FAN_CURVE_STATE_APPLIED 0x08
#define UW_FAN_CURVE_STATE_REVOKED 0x10
#define UW_FAN_CURVE_STATE_RESTORED_VERIFIED 0x20

/* Fixed 64-byte command header. ACQUIRE returns epoch/generation/lease_id.
 * APPLY and RELEASE require all three to match. request_id is caller chosen.
 * flags and all reserved fields must be zero. A lease_id of zero means none.
 */
struct uw_fan_curve_control_v1 {
	__u32 version;
	__u32 size;
	__u32 flags;
	__u32 reserved0;
	__aligned_u64 epoch;
	__aligned_u64 generation;
	__aligned_u64 lease_id;
	__aligned_u64 request_id;
	__u32 reserved[4];
};

/* Fixed 160 bytes. Each fan has 16 duty levels and 15 transitions. Temperatures
 * are firmware bytes, including existing sentinel values; duty_raw is 0..200
 * (percentage = raw/2). respective must be 0 or 1. No arbitrary EC addresses.
 */
struct uw_fan_curve_apply_v1 {
	struct uw_fan_curve_control_v1 control;
	__u8 rise[2][UW_FAN_CURVE_THRESHOLDS];
	__u8 fall[2][UW_FAN_CURVE_THRESHOLDS];
	__u8 duty_raw[2][UW_FAN_CURVE_LEVELS];
	__u8 respective;
	__u8 reserved[3];
};

/* Fixed 320 bytes. controls are 7c5,7c6,751,741,1804,1809 in that order.
 * original_targets are current fan0/fan1 and suspended fan0/fan1. They are
 * commands cached by this driver, not calibrated mechanical RPM. GET may
 * return metadata with LIVE_VALID clear when the transport is unavailable.
 */
struct uw_fan_curve_state_v1 {
	struct uw_fan_curve_control_v1 control;
	__u32 phase;
	__u32 capabilities;
	__u32 state_flags;
	__s32 live_error;
	__s32 last_error;
	__s32 restore_error;
	__u8 table[UW_FAN_CURVE_TABLE_BYTES];
	__u8 original_table[UW_FAN_CURVE_TABLE_BYTES];
	__u8 controls[6];
	__u8 original_controls[6];
	__u8 original_targets[4];
	__u32 reserved[6];
};

#define UW_FAN_CURVE_GET_STATE _IOWR(UW_FAN_CURVE_MAGIC, 0x00, struct uw_fan_curve_state_v1)
#define UW_FAN_CURVE_ACQUIRE _IOWR(UW_FAN_CURVE_MAGIC, 0x01, struct uw_fan_curve_control_v1)
#define UW_FAN_CURVE_APPLY _IOW(UW_FAN_CURVE_MAGIC, 0x02, struct uw_fan_curve_apply_v1)
#define UW_FAN_CURVE_RESTORE_RELEASE _IOW(UW_FAN_CURVE_MAGIC, 0x03, struct uw_fan_curve_control_v1)

#endif
