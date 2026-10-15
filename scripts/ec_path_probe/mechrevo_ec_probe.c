// SPDX-License-Identifier: GPL-2.0-only
/* Temporary, host-gated, read-only diagnostic. No EC write methods. */
#include <linux/acpi.h>
#include <linux/dmi.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/sysfs.h>
static acpi_handle inou;
static DEFINE_MUTEX(probe_lock);
static const u16 addresses[] = {
  0x043e, 0x044f, 0x0740, 0x0765, 0x0766,
  0x0464, 0x0465, 0x046b, 0x046c, 0x1804, 0x1809, 0x0e0d
};
static int read_byte(u16 addr, u64 *value)
{
  union acpi_object arg = { .type = ACPI_TYPE_INTEGER };
  struct acpi_object_list list = { .count = 1, .pointer = &arg };
  acpi_status status;
  arg.integer.value = addr;
  status = acpi_evaluate_integer(inou, "ECRR", &list, value);
  if (ACPI_FAILURE(status)) return -EIO;
  if (*value > 255) return -ERANGE;
  return 0;
}
static ssize_t snapshot_show(struct kobject *kobj, struct kobj_attribute *attr, char *buf)
{
  int i, ret;
  ssize_t used = 0;
  u64 value;
  mutex_lock(&probe_lock);
  for (i = 0; i < ARRAY_SIZE(addresses); ++i) {
    ret = read_byte(addresses[i], &value);
    if (ret) { mutex_unlock(&probe_lock); return ret; }
    used += sysfs_emit_at(buf, used, "%04x=%02llx\n", addresses[i], value);
  }
  mutex_unlock(&probe_lock);
  return used;
}
static struct kobj_attribute snapshot_attr = __ATTR(snapshot, 0400, snapshot_show, NULL);
static int __init probe_init(void)
{
  if (!dmi_match(DMI_SYS_VENDOR, "MECHREVO") ||
      !dmi_match(DMI_PRODUCT_NAME, "Kuangshi16Pro Series GM6PX0X") ||
      !dmi_match(DMI_BIOS_VERSION, "N.1.09MRO07")) return -ENODEV;
  if (ACPI_FAILURE(acpi_get_handle(NULL, "\\_SB.INOU", &inou)) ||
      !acpi_has_method(inou, "ECRR")) return -ENODEV;
  return sysfs_create_file(&THIS_MODULE->mkobj.kobj, &snapshot_attr.attr);
}
static void __exit probe_exit(void)
{
  sysfs_remove_file(&THIS_MODULE->mkobj.kobj, &snapshot_attr.attr);
}
module_init(probe_init);
module_exit(probe_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Temporary GM6PX0X MRO07 read-only ECRR comparison probe");
