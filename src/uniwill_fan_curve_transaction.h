/* SPDX-License-Identifier: GPL-2.0+ */
#ifndef UNIWILL_FAN_CURVE_TRANSACTION_H
#define UNIWILL_FAN_CURVE_TRANSACTION_H

/* The including provider supplies u8, u16, bool and errno definitions. This
 * header deliberately has no kernel services, allocation, permissions or PM
 * policy, so the exact engine can also run against a userspace mock EC.
 *
 * Callers hold the provider's command/state locks and exclude other table
 * owners. Callbacks return zero or a negative errno. update_bits must preserve
 * every bit outside clear_mask, atomically with other public EC transactions.
 * Neither software locks nor readback prove firmware/mechanical atomicity.
 */
#define UW_CURVE_TABLE_BYTES 96
#define UW_CURVE_LEVELS 16
#define UW_CURVE_THRESHOLDS 15
#define UW_CURVE_FANS 2

#define UW_CURVE_TABLE_BASE 0x0f00
#define UW_CURVE_SPLIT_ADDR 0x07c5
#define UW_CURVE_ENABLE_ADDR 0x07c6
#define UW_CURVE_MODE_ADDR 0x0751
#define UW_CURVE_SPLIT_MASK 0x80
#define UW_CURVE_ENABLE_MASK 0x04
#define UW_CURVE_FULL_MASK 0x40

struct uw_curve_io {
	void *ctx;
	int (*read)(void *ctx, u16 address, u8 *value);
	int (*write)(void *ctx, u16 address, u8 value);
	int (*update_bits)(void *ctx, u16 address, u8 clear_mask, u8 set_mask);
};

struct uw_curve_raw {
	u8 table[UW_CURVE_TABLE_BYTES];
	u8 ctrl7c5;
	u8 ctrl7c6;
	u8 mode751;
	bool valid;
};

/* rise/fall are the 15 encoded boundaries, not guessed firmware hysteresis
 * rules. Each fan has 16 raw duties (0..200). The terminal end byte is generated
 * as 0xff. Reserved down[0] bytes at 0x0f10/0x0f40 are never written.
 */
struct uw_curve_input {
	u8 rise[UW_CURVE_FANS][UW_CURVE_THRESHOLDS];
	u8 fall[UW_CURVE_FANS][UW_CURVE_THRESHOLDS];
	u8 duty_raw[UW_CURVE_FANS][UW_CURVE_LEVELS];
	u8 respective;
};

struct uw_curve_result {
	int original_errno;
	int restore_errno;
	bool snapshot_valid;
	bool write_attempted;
	bool applied_verified;
	bool restored_verified;
};

static inline bool uw_curve_io_valid(const struct uw_curve_io *io)
{
	return io && io->read && io->write && io->update_bits;
}

static inline bool uw_curve_reader_valid(const struct uw_curve_io *io)
{
	return io && io->read;
}

static inline int uw_curve_io_status(int status)
{
	/* A positive callback result violates the IO contract and is not success. */
	return status > 0 ? -EIO : status;
}

static inline int uw_curve_read(const struct uw_curve_io *io, u16 address,
			       u8 *value)
{
	return uw_curve_io_status(io->read(io->ctx, address, value));
}

static inline int uw_curve_require_not_full(const struct uw_curve_io *io)
{
	u8 mode;
	int status = uw_curve_read(io, UW_CURVE_MODE_ADDR, &mode);

	if (status)
		return status;
	return mode & UW_CURVE_FULL_MASK ? -EBUSY : 0;
}

/* All-or-nothing output validity; no write is performed, even on IO failure.
 * A raw snapshot is lossless. In particular, do not replace it with the OEM
 * getter's shifted/duplicated thresholds and integer-divided duty values.
 */
static inline int uw_curve_snapshot(const struct uw_curve_io *io,
				    struct uw_curve_raw *out)
{
	struct uw_curve_raw next = { 0 };
	int status;
	unsigned int i;

	if (!out)
		return -EINVAL;
	out->valid = false;
	if (!uw_curve_reader_valid(io))
		return -EINVAL;
	status = uw_curve_read(io, UW_CURVE_SPLIT_ADDR, &next.ctrl7c5);
	if (status)
		return status;
	status = uw_curve_read(io, UW_CURVE_ENABLE_ADDR, &next.ctrl7c6);
	if (status)
		return status;
	status = uw_curve_read(io, UW_CURVE_MODE_ADDR, &next.mode751);
	if (status)
		return status;
	for (i = 0; i < UW_CURVE_TABLE_BYTES; ++i) {
		status = uw_curve_read(io, UW_CURVE_TABLE_BASE + i, &next.table[i]);
		if (status)
			return status;
	}
	next.valid = true;
	*out = next;
	return 0;
}

/* Read-only stale-plan comparison. Unrelated 7c5/7c6 bit changes are allowed;
 * raw reserved bytes and the complete mode byte still participate in CAS.
 * This does not exclude writers outside the caller's locking protocol.
 */
static inline int uw_curve_compare(const struct uw_curve_io *io,
				   const struct uw_curve_raw *expected)
{
	struct uw_curve_raw observed_raw;
	unsigned int i;
	int status;

	if (!expected || !expected->valid)
		return -EINVAL;
	status = uw_curve_snapshot(io, &observed_raw);
	if (status)
		return status;
	if (((observed_raw.ctrl7c5 ^ expected->ctrl7c5) & UW_CURVE_SPLIT_MASK) ||
	    ((observed_raw.ctrl7c6 ^ expected->ctrl7c6) & UW_CURVE_ENABLE_MASK) ||
	    observed_raw.mode751 != expected->mode751)
		return -EAGAIN;
	for (i = 0; i < UW_CURVE_TABLE_BYTES; ++i)
		if (observed_raw.table[i] != expected->table[i])
			return -EAGAIN;
	return 0;
}

static inline int uw_curve_validate(const struct uw_curve_input *input)
{
	unsigned int fan, i;

	if (!input || input->respective > 1)
		return -EINVAL;
	/* The OEM enable predicate cannot enable an all-zero first CPU pair. */
	if (!input->rise[0][0] && !input->rise[0][1])
		return -EINVAL;
	for (fan = 0; fan < UW_CURVE_FANS; ++fan)
		for (i = 0; i < UW_CURVE_LEVELS; ++i)
			if (input->duty_raw[fan][i] > 200)
				return -EINVAL;
	/* No invented temperature limit, minimum duty, ordering or Down<=Up rule. */
	return 0;
}

/* Pure, lossless encoding for the provider's expected-state/CAS anchor. No
 * output is published on failure, and before == after is supported. The mode
 * byte remains diagnostic; the engine never writes it. The original lease
 * snapshot must remain separate from this expected active state.
 */
static inline int uw_curve_build_raw(const struct uw_curve_raw *before,
				     const struct uw_curve_input *input,
				     struct uw_curve_raw *after)
{
	struct uw_curve_raw next;
	unsigned int fan, i, base;
	int status;

	if (!before || !before->valid || !after)
		return -EINVAL;
	status = uw_curve_validate(input);
	if (status)
		return status;
	next = *before;
	for (fan = 0; fan < UW_CURVE_FANS; ++fan) {
		base = fan * 0x30;
		for (i = 0; i < UW_CURVE_LEVELS; ++i) {
			next.table[base + i] = i < UW_CURVE_THRESHOLDS ?
				input->rise[fan][i] : 0xff;
			if (i < UW_CURVE_THRESHOLDS)
				next.table[base + 0x11 + i] = input->fall[fan][i];
			next.table[base + 0x20 + i] = input->duty_raw[fan][i];
		}
	}
	next.ctrl7c5 = (next.ctrl7c5 & ~UW_CURVE_SPLIT_MASK) |
		(input->respective ? UW_CURVE_SPLIT_MASK : 0);
	next.ctrl7c6 |= UW_CURVE_ENABLE_MASK;
	*after = next;
	return 0;
}

static inline int uw_curve_verify_bits(const struct uw_curve_io *io,
				      u16 address, u8 mask, u8 bits)
{
	u8 value;
	int status = uw_curve_read(io, address, &value);

	if (status)
		return status;
	return (value & mask) == (bits & mask) ? 0 : -EIO;
}

static inline int uw_curve_update_verified(const struct uw_curve_io *io,
					   u16 address, u8 mask, u8 bits,
					   struct uw_curve_result *result)
{
	u8 value;
	int status = uw_curve_read(io, address, &value);

	if (status)
		return status;
	bits &= mask;
	if ((value & mask) == bits)
		return 0;
	/* Set before calling: an error may be returned after the write landed. */
	result->write_attempted = true;
	status = uw_curve_io_status(io->update_bits(io->ctx, address, mask, bits));
	if (status)
		return status;
	status = uw_curve_read(io, address, &value);
	if (status)
		return status;
	return (value & mask) == bits ? 0 : -EIO;
}

static inline int uw_curve_write_verified(const struct uw_curve_io *io,
					  u16 address, u8 target,
					  struct uw_curve_result *result)
{
	u8 value;
	int status;

	result->write_attempted = true;
	status = uw_curve_io_status(io->write(io->ctx, address, target));
	if (status)
		return status;
	status = uw_curve_read(io, address, &value);
	if (status)
		return status;
	return value == target ? 0 : -EIO;
}

/* Recovery-only quiescence must not be skipped because a preliminary read
 * failed. The atomic callback preserves unrelated bits and addresses only the
 * documented enable bit. Always attempt readback, even after a possibly landed
 * write error. Its caller retains the primary/restore errno independently.
 */
static inline int uw_curve_disable_best_effort(const struct uw_curve_io *io,
					      struct uw_curve_result *result)
{
	u8 value;
	int status, read_status;

	result->write_attempted = true;
	status = uw_curve_io_status(io->update_bits(io->ctx, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, 0));
	read_status = uw_curve_read(io, UW_CURVE_ENABLE_ADDR, &value);
	if (status)
		return status;
	if (read_status)
		return read_status;
	return value & UW_CURVE_ENABLE_MASK ? -EIO : 0;
}

static inline int uw_curve_verify_table(const struct uw_curve_io *io,
				       const struct uw_curve_raw *target)
{
	u8 value;
	unsigned int i;
	int status;

	/* Includes both reserved bytes: their external change cannot be repaired by
	 * inventing writes to addresses absent from the OEM normal write protocol. */
	for (i = 0; i < UW_CURVE_TABLE_BYTES; ++i) {
		status = uw_curve_read(io, UW_CURVE_TABLE_BASE + i, &value);
		if (status)
			return status;
		if (value != target->table[i])
			return -EIO;
	}
	return 0;
}

/* Completion requires a second whole-table read after enabling. Enabling may
 * itself trigger firmware activity, so the earlier disabled-table verification
 * alone cannot justify applied_verified/restored_verified.
 */
static inline int uw_curve_verify_state(const struct uw_curve_io *io,
				       const struct uw_curve_raw *target)
{
	int status = uw_curve_verify_table(io, target);

	if (status)
		return status;
	status = uw_curve_verify_bits(io, UW_CURVE_SPLIT_ADDR,
		UW_CURVE_SPLIT_MASK, target->ctrl7c5);
	if (status)
		return status;
	status = uw_curve_verify_bits(io, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, target->ctrl7c6);
	if (status)
		return status;
	return uw_curve_require_not_full(io);
}

static inline int uw_curve_write_table(const struct uw_curve_io *io,
				      const struct uw_curve_raw *target,
				      struct uw_curve_result *result)
{
	unsigned int fan, i, base;
	int status;

	for (fan = 0; fan < UW_CURVE_FANS; ++fan) {
		base = fan * 0x30;
		for (i = 0; i < UW_CURVE_LEVELS; ++i) {
			status = uw_curve_write_verified(io,
				UW_CURVE_TABLE_BASE + base + i,
				target->table[base + i], result);
			if (status)
				return status;
			if (i < UW_CURVE_THRESHOLDS) {
				status = uw_curve_write_verified(io,
					UW_CURVE_TABLE_BASE + base + 0x11 + i,
					target->table[base + 0x11 + i], result);
				if (status)
					return status;
			}
			status = uw_curve_write_verified(io,
				UW_CURVE_TABLE_BASE + base + 0x20 + i,
				target->table[base + 0x20 + i], result);
			if (status)
				return status;
		}
	}
	return 0;
}

/* No new-input policy is applied to old raw state: legacy/dummy tables may
 * have odd duties, non-0xff end bytes, or other values that must survive exact
 * restoration. Enable is attempted only after whole-table verification while
 * disabled; the public wrappers quiesce best-effort on restoration failure.
 */
static inline int uw_curve_restore_raw(const struct uw_curve_io *io,
				      const struct uw_curve_raw *before,
				      struct uw_curve_result *result)
{
	int status;

	status = uw_curve_require_not_full(io);
	if (status)
		return status;
	status = uw_curve_update_verified(io, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, 0, result);
	if (status)
		return status;
	status = uw_curve_require_not_full(io);
	if (status)
		return status;
	status = uw_curve_write_table(io, before, result);
	if (status)
		return status;
	status = uw_curve_verify_table(io, before);
	if (status)
		return status;
	status = uw_curve_update_verified(io, UW_CURVE_SPLIT_ADDR,
		UW_CURVE_SPLIT_MASK, before->ctrl7c5, result);
	if (status)
		return status;
	status = uw_curve_require_not_full(io);
	if (status)
		return status;
	status = uw_curve_verify_bits(io, UW_CURVE_SPLIT_ADDR,
		UW_CURVE_SPLIT_MASK, before->ctrl7c5);
	if (status)
		return status;
	/* Restore the original enable bit last, including an originally disabled
	 * table. AUTO would discard the original split/enable ownership instead. */
	status = uw_curve_update_verified(io, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, before->ctrl7c6, result);
	if (status)
		return status;
	return uw_curve_verify_state(io, before);
}

/* Explicit retry after a failed commit/restore. original_errno remains zero:
 * this entry did not perform an application; its status is restore_errno.
 * The caller retains both the failed application's errno and the raw snapshot.
 */
static inline int uw_curve_restore(const struct uw_curve_io *io,
				   const struct uw_curve_raw *before,
				   struct uw_curve_result *result)
{
	int status;

	if (!result)
		return -EINVAL;
	*result = (struct uw_curve_result) { 0 };
	if (!uw_curve_io_valid(io) || !before || !before->valid) {
		result->restore_errno = -EINVAL;
		return -EINVAL;
	}
	result->snapshot_valid = true;
	status = uw_curve_restore_raw(io, before, result);
	result->restore_errno = status;
	result->restored_verified = !status;
	/* A callback may have changed the enable bit or returned an error after
	 * landing a write. With failed transport quiescence cannot be promised;
	 * preserve the first restore errno and the retryable raw snapshot.
	 */
	if (status && result->write_attempted)
		(void)uw_curve_disable_best_effort(io, result);
	return status;
}

// @mark mechrevo.fan_curve role=implementation
static inline int uw_curve_commit(const struct uw_curve_io *io,
				  const struct uw_curve_input *input,
				  struct uw_curve_raw *before,
				  struct uw_curve_result *result)
{
	struct uw_curve_raw target;
	int status;

	if (!result)
		return -EINVAL;
	*result = (struct uw_curve_result) { 0 };
	if (!uw_curve_io_valid(io) || !before) {
		result->original_errno = -EINVAL;
		return -EINVAL;
	}
	status = uw_curve_validate(input);
	if (status)
		goto failed;
	status = uw_curve_snapshot(io, before);
	if (status)
		goto failed;
	result->snapshot_valid = true;
	status = uw_curve_build_raw(before, input, &target);
	if (status)
		goto failed;
	status = uw_curve_require_not_full(io);
	if (status)
		goto failed;
	status = uw_curve_update_verified(io, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, 0, result);
	if (status)
		goto failed;
	status = uw_curve_update_verified(io, UW_CURVE_SPLIT_ADDR,
		UW_CURVE_SPLIT_MASK, input->respective ? UW_CURVE_SPLIT_MASK : 0,
		result);
	if (status)
		goto failed;
	status = uw_curve_require_not_full(io);
	if (status)
		goto failed;
	status = uw_curve_write_table(io, &target, result);
	if (status)
		goto failed;
	status = uw_curve_verify_table(io, &target);
	if (status)
		goto failed;
	status = uw_curve_require_not_full(io);
	if (status)
		goto failed;
	status = uw_curve_update_verified(io, UW_CURVE_ENABLE_ADDR,
		UW_CURVE_ENABLE_MASK, UW_CURVE_ENABLE_MASK, result);
	if (status)
		goto failed;
	status = uw_curve_verify_state(io, &target);
	if (status)
		goto failed;
	result->applied_verified = true;
	return 0;

failed:
	result->original_errno = status;
	if (result->snapshot_valid && result->write_attempted) {
		result->restore_errno = uw_curve_restore_raw(io, before, result);
		result->restored_verified = !result->restore_errno;
		/* The application may already have enabled an unverified table, while
		 * restore failed before reaching its own disable. Do not let an initial
		 * rollback read failure prevent a separate atomic quiescence attempt.
		 */
		if (result->restore_errno)
			(void)uw_curve_disable_best_effort(io, result);
	}
	return status;
}

#endif /* UNIWILL_FAN_CURVE_TRANSACTION_H */
