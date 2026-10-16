"""Qualified, optional CPU0 temperature-reference diagnostics; no real hardware."""
import contextlib
import json
import os
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest import mock

from test_mechrevo_power import FakeHost, POWER


class TemperatureTargetDiagnostics(unittest.TestCase):
    def setUp(self):
        self.host = FakeHost(self)
        self.controller = self.host.controller
        self.host.write('/proc/cpuinfo', '\n'.join([
            'processor: 0', 'vendor_id: GenuineIntel', 'cpu family: 6', 'model: 151',
            'model name: '+POWER.CPU_MODEL, 'cpuid level: 32', 'flags: msr dtherm pts pln'])+'\n')
        self.host.write('/proc/sys/kernel/osrelease', '7.2.9-arch1-1\n')
        self.host.write('/proc/sys/kernel/random/boot_id', '11111111-2222-3333-4444-555555555555\n')
        self.host.write('/sys/module/msr/parameters/allow_writes', 'off\n')
        self.host.write('/sys/kernel/notes', 'qualified built-in kernel note')
        self.host.write('/usr/lib/modules/7.2.9-arch1-1/modules.builtin', 'kernel/arch/x86/kernel/msr.ko\n')
        self.node = self.host.path('/dev/cpu/0/msr')
        self.node.parent.mkdir(parents=True)
        self.set_msrs(1 << 30, (100 << 16) | (15 << 24))
        self.real_stat, self.real_fstat = os.stat, os.fstat
        self.real_pread, self.real_open, self.real_close = os.pread, os.open, os.close
        self.reads, self.opens, self.closes = [], [], []
        self.mutate_read = None
        self.node_identity = self.real_stat(self.node)

    def set_msrs(self, platform, target):
        with self.node.open('wb') as stream:
            stream.seek(0xce); stream.write(platform.to_bytes(8, 'little'))
            stream.seek(0x1a2); stream.write(target.to_bytes(8, 'little'))

    def character(self, info):
        return SimpleNamespace(st_mode=0o20600, st_dev=info.st_dev, st_ino=info.st_ino,
                               st_rdev=os.makedev(202, 0))

    @contextlib.contextmanager
    def msr_boundary(self):
        def node_stat(name, *args, **kwargs):
            value = self.real_stat(name, *args, **kwargs)
            return self.character(value) if name == 'msr' and kwargs.get('dir_fd') is not None else value
        def fstat(fd):
            value = self.real_fstat(fd)
            return self.character(value) if (value.st_dev, value.st_ino) == (self.node_identity.st_dev, self.node_identity.st_ino) else value
        def pread(fd, count, offset):
            self.reads.append((fd, count, offset))
            if self.mutate_read:
                return self.mutate_read(fd, count, offset)
            return self.real_pread(fd, count, offset)
        def opening(*args, **kwargs):
            fd = self.real_open(*args, **kwargs)
            self.opens.append((fd, args, kwargs))
            return fd
        def close(fd):
            self.closes.append(fd)
            return self.real_close(fd)
        with mock.patch.object(POWER.os, 'stat', node_stat), mock.patch.object(POWER.os, 'fstat', fstat), \
                mock.patch.object(POWER.os, 'pread', pread), mock.patch.object(POWER.os, 'open', opening), \
                mock.patch.object(POWER.os, 'close', close):
            yield

    def read(self):
        with self.msr_boundary():
            return self.host.env.read_temperature_target()

    def assert_no_mutation(self, method):
        with mock.patch.object(self.host.env, 'write_control', side_effect=AssertionError('hardware write')), \
                mock.patch.object(self.host.env, 'atomic', side_effect=AssertionError('configuration write')):
            return method()

    def test_platform_capability_and_six_bit_offset_decode_85c_without_sysfs_tcc_workaround_guard(self):
        value = self.assert_no_mutation(self.read)
        self.assertEqual(value['tjmax_celsius'], 100)
        self.assertEqual(value['tcc_offset_celsius'], 15)
        self.assertEqual(value['tcc_activation_celsius'], 85)
        self.assertEqual([offset for _, count, offset in self.reads], [0xce, 0x1a2])
        self.assertTrue(all(count == 8 for _, count, _ in self.reads))
        self.assertEqual(self.host.commands, [])
        self.assertEqual(sorted(fd for fd, _, _ in self.opens), sorted(self.closes))
        for _, args, kwargs in self.opens:
            self.assertEqual(args[1] & os.O_ACCMODE, os.O_RDONLY)
            self.assertTrue(args[1] & os.O_NOFOLLOW)
            self.assertTrue(args[1] & os.O_CLOEXEC)

    def test_offset_boundary_zero_and_63_with_upper_reserved_bits_excluded(self):
        for offset in (0, 5, 63):
            self.set_msrs(1 << 30, (100 << 16) | (offset << 24) | (3 << 30))
            value = self.read()
            self.assertEqual(value['tcc_offset_celsius'], offset)
            self.assertEqual(value['tcc_activation_celsius'], 100-offset)

    def test_absent_platform_capability_never_reads_target(self):
        self.set_msrs(0, (100 << 16) | (15 << 24))
        with self.assertRaisesRegex(POWER.PowerError, 'does not advertise'):
            self.read()
        self.assertEqual([o for _, _, o in self.reads], [0xce])
        self.assertEqual(sorted(fd for fd, _, _ in self.opens), sorted(self.closes))

    def test_unqualified_cpu_or_dmi_never_opens_a_device(self):
        original = self.host.read('/proc/cpuinfo')
        for bad in (original.replace('model: 151', 'model: 152'), original.replace('flags: msr dtherm pts pln','flags: msr dtherm pln'),
                    original.replace('cpuid level: 32','cpuid level: 5'), original+'\n'+original,
                    original.replace('vendor_id: GenuineIntel','vendor_id: AuthenticAMD')):
            self.host.write('/proc/cpuinfo', bad)
            with mock.patch.object(POWER.os, 'open') as opened:
                with self.assertRaises(POWER.PowerError): self.host.env.read_temperature_target()
                opened.assert_not_called()
        self.host.write('/proc/cpuinfo', original)
        self.host.write('/sys/class/dmi/id/bios_version', 'unknown')
        with mock.patch.object(POWER.os, 'open') as opened:
            with self.assertRaises(POWER.PowerError): self.host.env.read_temperature_target()
            opened.assert_not_called()

    def test_missing_or_loaded_only_msr_support_never_opens_nodes_or_autoloads(self):
        self.host.write('/usr/lib/modules/7.2.9-arch1-1/modules.builtin','other.ko\n')
        self.host.write('/sys/module/msr/notes/.note.gnu.build-id','loaded-only-note')
        with mock.patch.object(POWER.os, 'open') as opened:
            with self.assertRaisesRegex(POWER.PowerError, 'not proven built-in'): self.host.env.read_temperature_target()
            opened.assert_not_called()
        self.host.path('/sys/module/msr').rename(self.host.path('/sys/module/not-msr'))
        with mock.patch.object(POWER.os, 'open') as opened:
            with self.assertRaises(FileNotFoundError): self.host.env.read_temperature_target()
            opened.assert_not_called()
        self.assertEqual(self.host.commands, [])

    def test_loaded_initstate_refused_even_if_builtin_manifest_claims_support(self):
        self.host.write('/sys/module/msr/initstate','live\n')
        with mock.patch.object(POWER.os,'open') as opened:
            with self.assertRaisesRegex(POWER.PowerError,'loaded MSR module'):
                self.host.env.read_temperature_target()
            opened.assert_not_called()

    def test_parent_symlink_and_final_symlink_are_not_followed(self):
        self.node.parent.rename(self.node.parent.with_name('real-cpu0'))
        self.node.parent.symlink_to('real-cpu0', target_is_directory=True)
        with self.assertRaises(OSError): self.read()
        self.assertEqual(self.reads, [])
        self.node.parent.unlink();self.node.parent.with_name('real-cpu0').rename(self.node.parent)
        saved=self.node.with_name('saved');self.node.rename(saved);self.node.symlink_to(saved.name)
        # Test the real node type path, without the character-device fixture.
        with self.assertRaises(POWER.PowerError):self.host.env.read_temperature_target()
        self.assertEqual(self.reads, [])

    def test_regular_wrong_major_wrong_minor_nodes_are_refused_before_read(self):
        for mode, device in ((0o100600,os.makedev(202,0)),(0o20600,os.makedev(203,0)),(0o20600,os.makedev(202,1))):
            def character(info):
                return SimpleNamespace(st_mode=mode,st_dev=info.st_dev,st_ino=info.st_ino,st_rdev=device)
            with mock.patch.object(self,'character',character),self.assertRaises(POWER.PowerError):self.read()
        self.assertEqual(self.reads, [])

    def test_device_open_permission_failure_closes_already_open_parent_directories(self):
        original=self.real_open
        def denied(name,*args,**kwargs):
            if name=='msr':raise PermissionError('MSR open denied')
            return original(name,*args,**kwargs)
        self.real_open=denied
        with self.assertRaisesRegex(PermissionError,'MSR open denied'):self.read()
        self.assertEqual(len(self.opens),3)
        self.assertEqual(sorted(fd for fd,_,_ in self.opens),sorted(self.closes))
        self.assertEqual(self.reads,[])

    def test_opened_device_identity_change_is_refused_before_any_msr_read(self):
        original=self.real_fstat
        def replaced(fd):
            info=original(fd)
            if (info.st_dev,info.st_ino)==(self.node_identity.st_dev,self.node_identity.st_ino):
                return SimpleNamespace(st_mode=0o20600,st_dev=info.st_dev,st_ino=info.st_ino+1,st_rdev=os.makedev(202,0))
            return info
        self.real_fstat=replaced
        with self.assertRaisesRegex(POWER.PowerError,'changed during open'):self.read()
        self.assertEqual(self.reads,[])
        self.assertEqual(sorted(fd for fd,_,_ in self.opens),sorted(self.closes))

    def test_support_notes_manifest_and_initstate_changes_discard_read_values(self):
        changes=[('/sys/kernel/notes','changed kernel notes'),
                 ('/usr/lib/modules/7.2.9-arch1-1/modules.builtin','kernel/arch/x86/kernel/msr.ko\nextra.ko\n'),
                 ('/sys/module/msr/initstate','live')]
        for name,value in changes:
            with self.subTest(name=name):
                path=self.host.path(name);before=path.read_bytes() if path.exists() else None
                def changed(fd,count,offset):
                    if offset==0x1a2:self.host.write(name,value)
                    return self.real_pread(fd,count,offset)
                self.mutate_read=changed
                with self.assertRaises(POWER.PowerError):self.read()
                if before is None:path.unlink()
                else:path.write_bytes(before)

    def test_short_and_permission_reads_close_all_descriptors(self):
        for offset in (0xce,0x1a2):
            for failure in (b'bad',PermissionError('MSR access denied')):
                self.opens.clear();self.closes.clear()
                def read(fd,count,current):
                    if current==offset:
                        if isinstance(failure,Exception):raise failure
                        return failure
                    return self.real_pread(fd,count,current)
                self.mutate_read=read
                with self.assertRaises((POWER.PowerError,PermissionError)):self.read()
                self.assertEqual(sorted(fd for fd,_,_ in self.opens),sorted(self.closes))

    def test_close_failure_still_attempts_every_descriptor_and_reports_prior_read_error(self):
        self.mutate_read=lambda fd,count,offset:b'bad'
        original_close=self.real_close
        def reported_failure(fd):
            original_close(fd)
            if fd==self.opens[-1][0]:raise OSError('close-report failure')
        self.real_close=reported_failure
        with self.assertRaisesRegex(POWER.PowerError,'close-report failure.*short MSR read'):
            self.read()
        self.assertEqual(sorted(fd for fd,_,_ in self.opens),sorted(self.closes))

    def test_boot_and_parent_identity_changes_discard_the_reference(self):
        def change_boot(fd,count,offset):
            if offset==0x1a2:self.host.write('/proc/sys/kernel/random/boot_id','aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee')
            return self.real_pread(fd,count,offset)
        self.mutate_read=change_boot
        with self.assertRaisesRegex(POWER.PowerError,'identity changed'):self.read()
        self.host.write('/proc/sys/kernel/random/boot_id','11111111-2222-3333-4444-555555555555')
        def change_parent(fd,count,offset):
            if offset==0x1a2:self.node.parent.rename(self.node.parent.with_name('old-parent'));self.node.parent.mkdir()
            return self.real_pread(fd,count,offset)
        self.mutate_read=change_parent
        with self.assertRaisesRegex(POWER.PowerError,'parent directory changed'):self.read()

    def test_invalid_temperature_reference_not_decoded_as_zero(self):
        for tjmax,offset in ((0,0),(99,0),(101,0),(126,0),(30,30),(20,63)):
            self.set_msrs(1<<30,(tjmax<<16)|(offset<<24))
            with self.assertRaisesRegex(POWER.PowerError,'unqualified TjMax'):self.read()

    def test_managed_status_reference_failure_is_optional_and_software_target_is_separate(self):
        self.host.write('/sys/class/thermal/thermal_zone0/type','x86_pkg_temp')
        self.host.write('/sys/class/thermal/thermal_zone0/temp','65000')
        self.controller.set_limit(55,target_celsius=90,polling_seconds=1,rapl_debounce_seconds=0)
        before={p:self.host.path(p).read_bytes() for p in (POWER.XML,POWER.DROPIN,POWER.STATE)}
        self.host.commands.clear()
        with self.msr_boundary():result=self.assert_no_mutation(self.controller.status)
        target=result['temperature_target']
        self.assertTrue(result['holding_now']);self.assertEqual(result['errors'],[])
        self.assertEqual(result['holding_scope'],'cpu_package_pl1')
        self.assertTrue(target['available']);self.assertEqual(target['software_target_celsius'],90)
        self.assertEqual(target['tcc_activation_celsius'],85);self.assertFalse(target['software_precedes_hardware'])
        self.set_msrs(0,100<<16)
        with self.msr_boundary():result=self.assert_no_mutation(self.controller.status)
        self.assertTrue(result['holding_now']);self.assertEqual(result['errors'],[])
        self.assertFalse(result['temperature_target']['available']);self.assertIsNone(result['temperature_target']['tcc_activation_celsius'])
        self.assertTrue(any('PLATFORM_INFO' in e for e in result['diagnostic_errors']))
        self.assertEqual(before,{p:self.host.path(p).read_bytes() for p in before})
        self.assertTrue(all(a[0] in ('systemctl','/usr/bin/thermald') for a in self.host.commands))

    def test_unmanaged_status_has_no_software_target_and_read_failures_keep_cli_healthy(self):
        with self.msr_boundary():result=self.assert_no_mutation(self.controller.status)
        self.assertFalse(result['managed']);self.assertEqual(result['errors'],[])
        self.assertTrue(result['temperature_target']['available']);self.assertIsNone(result['temperature_target']['software_target_celsius'])
        self.assertIsNone(result['temperature_target']['software_precedes_hardware'])
        with mock.patch.object(POWER,'PowerController',return_value=self.controller),mock.patch('sys.stdout'), \
                mock.patch.object(self.host.env,'read_temperature_target',side_effect=PermissionError('no MSR permission')):
            self.assertEqual(POWER.main(['status']),0)
        self.assertTrue(all(a[0]=='systemctl' for a in self.host.commands))


if __name__=='__main__':unittest.main()
