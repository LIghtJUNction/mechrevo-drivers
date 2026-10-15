"""Execute the actual kernel probe C against bounded ACPI/sysfs test doubles."""
import ctypes
from pathlib import Path
import subprocess
import tempfile
import unittest


STUBS = r'''
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <stdarg.h>
#include <errno.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef unsigned long long u64;
typedef void *acpi_handle;
#define U32_MAX UINT32_MAX
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_BUFFER 3
#define ACPI_ALLOCATE_BUFFER ((size_t)-1)
#define ACPI_FAILURE(s) ((s) != 0)
#define DEFINE_MUTEX(n) int n
#define __init
#define __exit
#define module_init(n)
#define module_exit(n)
#define MODULE_LICENSE(n)
#define MODULE_DESCRIPTION(n)
#define DMI_SYS_VENDOR 1
#define DMI_PRODUCT_NAME 2
#define DMI_BIOS_VERSION 3
struct kobject { int unused; };
struct attribute { int mode; };
struct kobj_attribute { struct attribute attr; };
struct module { struct { struct kobject kobj; } mkobj; };
static struct module fake_module;
#define THIS_MODULE (&fake_module)
#define __ATTR(n,m,s,w) { .attr = {m} }
union acpi_object {
 int type;
 struct { int type; u64 value; } integer;
 struct { int type; size_t length; u8 *pointer; } buffer;
};
struct acpi_object_list { unsigned count; union acpi_object *pointer; };
struct acpi_buffer { size_t length; void *pointer; };
static int failure, depth, smrw_reads, invalid_requests, creates, removes;
static int bad_dmi, bad_handle, bad_method, bad_sysfs;
static u64 current_phase, current_gpu;
static char result[4096];
static void mutex_lock(int *m) { (void)m; ++depth; }
static void mutex_unlock(int *m) { (void)m; --depth; }
static int sysfs_emit_at(char *buf, int pos, const char *fmt, ...) {
 va_list args; int n; va_start(args,fmt); n=vsnprintf(buf+pos,4096-pos,fmt,args); va_end(args); return n;
}
#define sysfs_emit(b,...) sysfs_emit_at(b,0,__VA_ARGS__)
#define kfree(p) free(p)
static const u8 factory[12]={130,130,200,5,0,25,87,125,255,255,255,255};
static int acpi_evaluate_integer(acpi_handle h, const char *name, struct acpi_object_list *a, u64 *v) {
 (void)h;
 if (!strcmp(name,"\\PPFG")) { if(failure==1)return 1; *v=current_phase; return 0; }
 if (!strcmp(name,"ECRR")) {
  if(!a || a->count!=1 || a->pointer->type!=ACPI_TYPE_INTEGER)return 1;
  if(a->pointer->integer.value==0x7d2) { if(failure==2)return 1; *v=current_gpu; return 0; }
  if(a->pointer->integer.value==0x751) { if(failure==3)return 1; *v=0; return 0; }
  ++invalid_requests; return 1;
 }
 if (!strcmp(name,"SMRW")) {
  int offset;
  if(!a || a->count!=1 || a->pointer->type!=ACPI_TYPE_BUFFER || a->pointer->buffer.length!=13) {++invalid_requests;return 1;}
  u8 *b=a->pointer->buffer.pointer;
  if(b[0]!=0xbb || (b[1]!=0 && b[1]!=4 && b[1]!=8)) {++invalid_requests;return 1;}
  for(int i=2;i<13;i++)if(b[i]) {++invalid_requests;return 1;}
  ++smrw_reads;
  if(failure==8)return 1;
  offset=b[1]; *v=(u32)factory[offset] | ((u32)factory[offset+1]<<8) | ((u32)factory[offset+2]<<16) | ((u32)factory[offset+3]<<24);
  if(failure==9)*v=0x100000000ULL;
  if(failure==10)*v^=1;
  return 0;
 }
 ++invalid_requests; return 1;
}
static int acpi_evaluate_object(acpi_handle h, char *name, void *args, struct acpi_buffer *out) {
 (void)h; (void)name; (void)args;
 if(failure==4)return 1;
 union acpi_object *o=calloc(1,sizeof(*o)+12);
 o->buffer.type=failure==5 ? ACPI_TYPE_INTEGER : ACPI_TYPE_BUFFER;
 o->buffer.length=failure==6 ? 11 : 12;
 o->buffer.pointer=failure==7 ? NULL : (u8 *)(o+1);
 if(o->buffer.pointer)memcpy(o->buffer.pointer,factory,12);
 out->pointer=o; return 0;
}
static int dmi_match(int field, const char *wanted) {
 const char *values[]={"", "MECHREVO", "Kuangshi16Pro Series GM6PX0X", "N.1.09MRO07"};
 return field!=bad_dmi && !strcmp(values[field],wanted);
}
static int acpi_get_handle(void *parent, const char *name, acpi_handle *out) {
 (void)parent; if(bad_handle || strcmp(name,"\\_SB.INOU"))return 1; *out=(void *)1; return 0;
}
static int acpi_has_method(acpi_handle h, const char *name) { (void)h; return !bad_method && !strcmp(name,"ECRR"); }
static int sysfs_create_file(struct kobject *k, struct attribute *a) { (void)k; ++creates; return bad_sysfs ? -ENOMEM : a->mode==0400 ? 0 : -EINVAL; }
static void sysfs_remove_file(struct kobject *k, struct attribute *a) { (void)k; (void)a; ++removes; }
'''

WRAPPERS = r'''
int run_snapshot(int fail, u64 phase, u64 gpu) {
 failure=fail; current_phase=phase; current_gpu=gpu;
 depth=smrw_reads=invalid_requests=0; memset(result,0,sizeof(result));
 return snapshot_show(NULL,NULL,result);
}
const char *output_text(void) { return result; }
int lock_depth(void) { return depth; }
int read_count(void) { return smrw_reads; }
int invalid_count(void) { return invalid_requests; }
int run_init(int fail) {
 bad_dmi=fail<=3?fail:0; bad_handle=fail==4; bad_method=fail==5; bad_sysfs=fail==6;
 creates=removes=0;
 int r=probe_init(); if(!r)probe_exit(); return r;
}
int create_count(void) { return creates; }
int remove_count(void) { return removes; }
'''


class FactoryProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.tmp.cleanup)
        source = (Path(__file__).resolve().parents[1] /
                  "scripts/factory_policy_probe/mechrevo_policy_probe.c").read_text()
        source = "\n".join(line for line in source.splitlines() if not line.startswith("#include"))
        path = Path(cls.tmp.name)
        (path / "test.c").write_text(STUBS + source + WRAPPERS)
        subprocess.run(["gcc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
                        "-Wno-unused-parameter", "-o", str(path / "probe.so"),
                        str(path / "test.c")], check=True, capture_output=True)
        cls.lib = ctypes.CDLL(str(path / "probe.so"))
        cls.lib.run_snapshot.argtypes = (ctypes.c_int, ctypes.c_ulonglong, ctypes.c_ulonglong)
        cls.lib.output_text.restype = ctypes.c_char_p

    def run_snapshot(self, fail=0, phase=4, gpu=0x11):
        value = self.lib.run_snapshot(fail, phase, gpu)
        self.assertEqual(self.lib.lock_depth(), 0)
        self.assertEqual(self.lib.invalid_count(), 0)
        return value, self.lib.output_text().decode()

    def test_actual_c_fixed_read_packet_and_dword_comparison(self):
        value, text = self.run_snapshot()
        self.assertGreater(value, 0)
        self.assertIn("bytes=8282c8050019577dffffffff", text)
        self.assertIn("smrw_0=05c88282", text)
        self.assertIn("smrw_4=7d571900", text)
        self.assertIn("smrw_8=ffffffff", text)
        self.assertEqual(self.lib.read_count(), 3)

    def test_all_nine_firmware_selection_branches(self):
        for phase in (2, 3, 4):
            for gpu, name in ((9, "AP02"), (0x11, f"AX4{phase}"), (0x12, f"AX6{phase}")):
                with self.subTest(phase=phase, gpu=gpu):
                    value, text = self.run_snapshot(phase=phase, gpu=gpu)
                    self.assertGreater(value, 0)
                    self.assertIn("table=" + name + "\n", text)

    def test_unsupported_identity_never_calls_smrw(self):
        for phase, gpu in ((0, 0x11), (1, 0x11), (5, 0x11), (4, 0x10), (4, 255)):
            with self.subTest(phase=phase, gpu=gpu):
                value, text = self.run_snapshot(phase=phase, gpu=gpu)
                self.assertGreater(value, 0)
                self.assertIn("table=unsupported", text)
                self.assertEqual(self.lib.read_count(), 0)

    def test_acpi_failures_invalid_objects_and_mismatches_fail_snapshot(self):
        for failure in range(1, 11):
            with self.subTest(failure=failure):
                value, _ = self.run_snapshot(fail=failure)
                self.assertLess(value, 0)

    def test_non_byte_identity_rejected(self):
        value, _ = self.run_snapshot(gpu=256)
        self.assertEqual(value, -34)  # ERANGE
        self.assertEqual(self.lib.read_count(), 0)

    def test_exact_host_and_method_gates_and_sysfs_failure(self):
        self.assertEqual(self.lib.run_init(0), 0)
        self.assertEqual(self.lib.create_count(), 1)
        self.assertEqual(self.lib.remove_count(), 1)
        for failure in range(1, 6):
            with self.subTest(failure=failure):
                self.assertEqual(self.lib.run_init(failure), -19)  # ENODEV
                self.assertEqual(self.lib.create_count(), 0)
        self.assertEqual(self.lib.run_init(6), -12)  # ENOMEM
        self.assertEqual(self.lib.remove_count(), 0)


if __name__ == "__main__":
    unittest.main()
