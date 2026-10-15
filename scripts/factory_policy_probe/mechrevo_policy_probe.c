// SPDX-License-Identifier: GPL-2.0-only
/* Fixed, host-gated, root-only factory-policy investigation.
 * ECRR reads and SMRW 0xbb reads only. No configurable arguments or write API.
 */
#include <linux/acpi.h>
#include <linux/dmi.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/sysfs.h>
static acpi_handle inou;
static DEFINE_MUTEX(probe_lock);
static int byte_at(u16 addr, u64 *value)
{
 union acpi_object arg = { .type = ACPI_TYPE_INTEGER };
 struct acpi_object_list args = { .count = 1, .pointer = &arg };
 arg.integer.value = addr;
 if (ACPI_FAILURE(acpi_evaluate_integer(inou, "ECRR", &args, value))) return -EIO;
 return *value <= 255 ? 0 : -ERANGE;
}
static const char *table_for(u64 phase, u64 gpu)
{
 if (phase < 2 || phase > 4) return NULL;
 if (gpu == 0x09) return "AP02"; /* MRO07 phase 3/4 also use AP02, not AP03. */
 if (gpu == 0x11) return phase == 2 ? "AX42" : phase == 3 ? "AX43" : "AX44";
 if (gpu == 0x12) return phase == 2 ? "AX62" : phase == 3 ? "AX63" : "AX64";
 return NULL;
}
static ssize_t snapshot_show(struct kobject *k, struct kobj_attribute *a, char *buf)
{
 struct acpi_buffer output = { ACPI_ALLOCATE_BUFFER, NULL };
 union acpi_object *obj;
 u64 phase, gpu, mode;
 const char *name;
 ssize_t used;
 int ret, i;
 mutex_lock(&probe_lock);
 if (ACPI_FAILURE(acpi_evaluate_integer(NULL, "\\PPFG", NULL, &phase))) { ret = -EIO; goto out; }
 ret = byte_at(0x07d2, &gpu); if (ret) goto out;
 ret = byte_at(0x0751, &mode); if (ret) goto out;
 used = sysfs_emit(buf, "phase=%llu\ngfid=%02llx\nmode=%02llx\n", phase, gpu, mode);
 name = table_for(phase, gpu);
 if (!name) { used += sysfs_emit_at(buf, used, "table=unsupported\n"); ret = used; goto out; }
 if (ACPI_FAILURE(acpi_evaluate_object(inou, (char *)name, NULL, &output))) { ret = -EIO; goto free; }
 obj = output.pointer;
 if (!obj || obj->type != ACPI_TYPE_BUFFER || obj->buffer.length != 12 || !obj->buffer.pointer) { ret = -EINVAL; goto free; }
 used += sysfs_emit_at(buf, used, "table=%s\nbytes=", name);
 for (i = 0; i < 12; ++i) used += sysfs_emit_at(buf, used, "%02x", obj->buffer.pointer[i]);
 used += sysfs_emit_at(buf, used, "\n");
 /* Fixed read command only; 13 bytes cover all fields declared by MRO07 SMRW. */
 for (i = 0; i < 3; ++i) {
  u8 request[13] = { 0xbb };
  union acpi_object arg = { .type = ACPI_TYPE_BUFFER };
  struct acpi_object_list args = { .count = 1, .pointer = &arg };
  u64 word;
  const u8 *b = obj->buffer.pointer + 4 * i;
  u32 expected = (u32)b[0] | ((u32)b[1] << 8) | ((u32)b[2] << 16) | ((u32)b[3] << 24);
  request[1] = i * 4;
  arg.buffer.pointer = request;
  arg.buffer.length = sizeof(request);
  if (ACPI_FAILURE(acpi_evaluate_integer(inou, "SMRW", &args, &word)) || word > U32_MAX || word != expected) { ret = -EIO; goto free; }
  used += sysfs_emit_at(buf, used, "smrw_%d=%08llx\n", i * 4, word);
 }
 ret = used;
free:
 kfree(output.pointer);
out:
 mutex_unlock(&probe_lock);
 return ret;
}
static struct kobj_attribute snapshot_attr = __ATTR(snapshot, 0400, snapshot_show, NULL);
static int __init probe_init(void)
{
 if (!dmi_match(DMI_SYS_VENDOR, "MECHREVO") || !dmi_match(DMI_PRODUCT_NAME, "Kuangshi16Pro Series GM6PX0X") || !dmi_match(DMI_BIOS_VERSION, "N.1.09MRO07")) return -ENODEV;
 if (ACPI_FAILURE(acpi_get_handle(NULL, "\\_SB.INOU", &inou)) || !acpi_has_method(inou, "ECRR")) return -ENODEV;
 return sysfs_create_file(&THIS_MODULE->mkobj.kobj, &snapshot_attr.attr);
}
static void __exit probe_exit(void) { sysfs_remove_file(&THIS_MODULE->mkobj.kobj, &snapshot_attr.attr); }
module_init(probe_init);
module_exit(probe_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("GM6PX0X MRO07 read-only factory policy probe");
