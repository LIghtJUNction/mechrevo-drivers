// SPDX-License-Identifier: GPL-2.0-only
/*
 * Temporary, read-only ECRR experiment for GM6PX0X / N.1.09MRO07.
 * No ECRW, EC writes, fan control or uniwill_laptop dependency.
 */
#include <linux/acpi.h>
#include <linux/delay.h>
#include <linux/dmi.h>
#include <linux/errno.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/sysfs.h>

#define ECRR_DELAY_MIN_US 6000
#define ECRR_DELAY_MAX_US 12000
#define MAX_ATTEMPTS 3

static acpi_handle inou;
static DEFINE_MUTEX(sample_lock);
static unsigned int sample_sequence;

static int ecrr_byte(u16 address, u8 *value)
{
	union acpi_object argument = {
		.integer.type = ACPI_TYPE_INTEGER,
		.integer.value = address,
	};
	struct acpi_object_list arguments = {
		.count = 1,
		.pointer = &argument,
	};
	unsigned long long raw;
	acpi_status status;

	status = acpi_evaluate_integer(inou, "ECRR", &arguments, &raw);
	if (ACPI_FAILURE(status))
		return -EIO;

	usleep_range(ECRR_DELAY_MIN_US, ECRR_DELAY_MAX_US);
	if (raw > 0xff)
		return -ERANGE;

	*value = raw;
	return 0;
}

/*
 * Both low-byte candidates are bracketed by the same high-byte reads.
 * Alternate their order between samples to expose systematic timing bias.
 * Report failed rollover attempts too; userspace must not treat them as RPM.
 */
static ssize_t sample_show(struct kobject *kobj,
			   struct kobj_attribute *attr, char *buf)
{
	u8 before, after, low_b, low_d;
	bool b_first, stable;
	ssize_t used = 0;
	int attempt, ret;

	mutex_lock(&sample_lock);
	b_first = !(sample_sequence++ & 1);
	for (attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
		ret = ecrr_byte(0x046c, &before);
		if (ret)
			goto unlock;
		ret = ecrr_byte(b_first ? 0x046b : 0x046d,
				b_first ? &low_b : &low_d);
		if (ret)
			goto unlock;
		ret = ecrr_byte(b_first ? 0x046d : 0x046b,
				b_first ? &low_d : &low_b);
		if (ret)
			goto unlock;
		ret = ecrr_byte(0x046c, &after);
		if (ret)
			goto unlock;

		stable = before == after;
		used += sysfs_emit_at(buf, used,
			"attempt=%d order=%s hi0=%02x b=%02x d=%02x hi1=%02x stable=%d\n",
			attempt, b_first ? "BD" : "DB",
			before, low_b, low_d, after, stable);
		if (stable)
			break;
	}
	ret = 0;
unlock:
	mutex_unlock(&sample_lock);
	return ret ? ret : used;
}

static struct kobj_attribute sample_attr =
	__ATTR(sample, 0400, sample_show, NULL);

static int __init fan2_probe_init(void)
{
	u8 project, capability;
	int ret;

	/* Exact host/board/BIOS gate, independent of a force parameter. */
	if (!dmi_match(DMI_SYS_VENDOR, "MECHREVO") ||
	    !dmi_match(DMI_PRODUCT_NAME, "Kuangshi16Pro Series GM6PX0X") ||
	    !dmi_match(DMI_BOARD_NAME, "GM6PX0X") ||
	    !dmi_match(DMI_BIOS_VERSION, "N.1.09MRO07"))
		return -ENODEV;

	if (ACPI_FAILURE(acpi_get_handle(NULL, "\\\\_SB.INOU", &inou)) ||
	    !acpi_has_method(inou, "ECRR"))
		return -ENODEV;

	ret = ecrr_byte(0x0740, &project);
	if (ret)
		return ret;
	ret = ecrr_byte(0x078e, &capability);
	if (ret)
		return ret;
	if (project != 0x10 || !(capability & 0x40))
		return -ENODEV;

	return sysfs_create_file(&THIS_MODULE->mkobj.kobj, &sample_attr.attr);
}

static void __exit fan2_probe_exit(void)
{
	sysfs_remove_file(&THIS_MODULE->mkobj.kobj, &sample_attr.attr);
}

module_init(fan2_probe_init);
module_exit(fan2_probe_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Read-only GM6PX0X second-fan ECRR offset comparison");
