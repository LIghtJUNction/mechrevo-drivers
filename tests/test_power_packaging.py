"""Build the optional power-control package locally; never install or enable it."""

import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
PACKAGING = ROOT / "packaging/arch/mechrevo-power-control"
PKGNAME = "mechrevo-power-control"
PAYLOAD = {
    "scripts/mechrevo-power": ("usr/bin/mechrevo-power", b"#!/usr/bin/python3\n# frozen launcher\n", 0o755),
    "scripts/mechrevo_power.py": (
        "usr/lib/mechrevo-power-control/mechrevo_power.py", b"# frozen module\n", 0o644,
    ),
    "scripts/mechrevo-power-console-helper": (
        "usr/lib/mechrevo-power-control/console-helper", b"#!/usr/bin/python3 -I\n# frozen helper\n", 0o755,
    ),
    "console/resources/org.mechrevo.console.cpu-power.policy": (
        "usr/share/polkit-1/actions/org.mechrevo.console.cpu-power.policy",
        (ROOT / "console/resources/org.mechrevo.console.cpu-power.policy").read_bytes(), 0o644,
    ),
    "scripts/mechrevo-thermal": ("usr/bin/mechrevo-thermal", b"#!/usr/bin/python3\n# frozen observer launcher\n", 0o755),
    "scripts/mechrevo_thermal.py": (
        "usr/lib/mechrevo-power-control/mechrevo_thermal.py", b"# frozen observer\n", 0o644,
    ),
    "scripts/mechrevo-peak-trial": (
        "usr/bin/mechrevo-peak-trial", b"#!/usr/bin/python3\n# frozen peak trial launcher\n", 0o755,
    ),
    "scripts/peak_limit_trial.py": (
        "usr/lib/mechrevo-power-control/peak_limit_trial.py", b"# frozen peak trial\n", 0o644,
    ),
    "docs/POWER-CONTROL.md": (
        "usr/share/doc/mechrevo-power-control/POWER-CONTROL.md", b"Frozen documentation.\n", 0o644,
    ),
    "docs/THERMAL-OBSERVATION.md": (
        "usr/share/doc/mechrevo-power-control/THERMAL-OBSERVATION.md", b"Frozen observer documentation.\n", 0o644,
    ),
    "docs/PEAK-LIMIT-TRIAL.md": (
        "usr/share/doc/mechrevo-power-control/PEAK-LIMIT-TRIAL.md", b"Frozen peak trial documentation.\n", 0o644,
    ),
    "LICENSE": (
        "usr/share/licenses/mechrevo-power-control/LICENSE", b"GPL-2.0-or-later fixture\n", 0o644,
    ),
}


@unittest.skipUnless(all(shutil.which(tool) for tool in ("makepkg", "bsdtar", "git", "xmllint", "fakeroot")),
                     "Arch packaging tools are required")
class PowerPackaging(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="power-packaging-", dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.source = self.path / "source-repo"
        self.source.mkdir()
        self.git("init", "-b", "main")
        for key, value in (("user.name", "Packaging test"),
                           ("user.email", "test@example.invalid"),
                           ("commit.gpgsign", "false")):
            self.git("config", key, value)
        for relative, (_, content, _) in PAYLOAD.items():
            target = self.source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        self.git("add", ".")
        self.git("commit", "-m", "Frozen fixture")
        self.commit = self.git("rev-parse", "HEAD")
        self.build = self.path / "recipe"
        self.build.mkdir()
        shutil.copy2(PACKAGING / "PKGBUILD", self.build / "PKGBUILD")
        # Keep makepkg's build, source, log and output writes within the tracked root.
        # The local machine has a /tmp BUILDDIR; do not inherit that global setting.
        self.config = self.path / "makepkg.conf"
        self.config.write_text(
            "source /etc/makepkg.conf\n"
            'BUILDDIR="$startdir/build"\n'
            'PKGDEST="$startdir"\n'
            'SRCDEST="$startdir"\n'
            'SRCPKGDEST="$startdir"\n'
            'LOGDEST="$startdir"\n'
            "BUILDENV=(!distcc !color !ccache check !sign)\n"
        )
        self.env = os.environ.copy()
        self.env.update(MECHREVO_POWER_COMMIT=self.commit,
                        MECHREVO_POWER_SOURCE=self.source.as_uri(),
                        TMPDIR=str(self.path))
        self.staging = self.path / "staging"
        self.staging.mkdir()

    def git(self, *args):
        result = subprocess.run(["git", "-C", str(self.source), *args],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout.strip()

    def makepkg(self, *args, env=None):
        return subprocess.run(["makepkg", "--config", str(self.config), *args],
                              cwd=self.build, env=self.env if env is None else env,
                              text=True, capture_output=True)

    def metadata(self):
        result = self.makepkg("--printsrcinfo")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        metadata = {}
        for line in result.stdout.splitlines():
            if line.strip():
                key, value = line.strip().split(" = ", 1)
                metadata.setdefault(key, []).append(value)
        return metadata

    def prepare(self):
        return self.phase("prepare")

    def phase(self, name, *, fakeroot=False, errexit=True):
        srcdir = self.path / "prepare"
        srcdir.mkdir(exist_ok=True)
        checkout = srcdir / "mechrevo-drivers"
        if not checkout.exists():
            checkout.symlink_to(self.source, target_is_directory=True)
        return subprocess.run(
            (["fakeroot", "--"] if fakeroot else []) +
            ["bash", *(["-e"] if errexit else []), "-c", 'source "$1"; srcdir="$2"; pkgdir="$3"; "$4"',
             "power-packaging-test", str(self.build / "PKGBUILD"), str(srcdir),
             str(self.staging), name],
            env=self.env, text=True, capture_output=True,
        )

    def test_metadata_requires_qualified_thermald_without_driver_dependency(self):
        metadata = self.metadata()
        self.assertEqual(metadata["pkgbase"], [PKGNAME])
        self.assertEqual(metadata["pkgname"], [PKGNAME])
        self.assertEqual(metadata["pkgver"], ["0.1.0"])
        self.assertEqual(metadata["pkgrel"], ["18"])
        self.assertEqual(metadata["arch"], ["x86_64"])
        self.assertEqual(metadata["depends"], ["python", "thermald=2:2.5.13", "systemd", "polkit"])
        self.assertEqual(metadata["makedepends"], ["git", "libxml2"])
        self.assertEqual(metadata["license"], ["GPL-2.0-or-later"])
        self.assertEqual(metadata["source"], [
            f"mechrevo-drivers::git+{self.source.as_uri()}#commit={self.commit}",
        ])
        for key in ("install", "conflicts", "replaces", "backup"):
            self.assertNotIn(key, metadata)
        self.assertEqual(metadata["sha256sums"], ["SKIP"])

    def test_commit_must_be_full_immutable_hash(self):
        for commit in ("", "main", "v0.1.0", "abcdef", "a" * 39, "a" * 41, "g" * 40):
            with self.subTest(commit=commit):
                env = dict(self.env, MECHREVO_POWER_COMMIT=commit)
                result = self.makepkg("--printsrcinfo", env=env)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("reviewed full 40-character Git commit", result.stderr)

    def test_prepare_rejects_checkout_that_does_not_match_pin(self):
        self.env["MECHREVO_POWER_COMMIT"] = "a" * 40
        result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match", result.stderr)

    def test_prepare_rejects_missing_required_source(self):
        for relative, (_, content, _) in PAYLOAD.items():
            with self.subTest(relative=relative):
                source = self.source / relative
                source.unlink()
                try:
                    result = self.prepare()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Required regular source file missing: " + relative, result.stderr)
                finally:
                    source.write_bytes(content)

    def test_prepare_rejects_source_file_symlink(self):
        for relative, (_, content, _) in PAYLOAD.items():
            with self.subTest(relative=relative):
                source = self.source / relative
                source.unlink()
                source.symlink_to(self.config)
                try:
                    result = self.prepare()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Required regular source file missing: " + relative, result.stderr)
                finally:
                    source.unlink()
                    source.write_bytes(content)

    def commit_current(self):
        self.git("add", "--all")
        self.git("commit", "-m", "Reviewed fixture")
        self.commit = self.git("rev-parse", "HEAD")
        self.env["MECHREVO_POWER_COMMIT"] = self.commit

    def test_all_stages_reject_dirty_existing_source(self):
        (self.source / "scripts/mechrevo_power.py").write_bytes(b"# unreviewed coordinator\n")
        for name in ("prepare", "check", "package"):
            with self.subTest(name=name):
                result = self.phase(name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unchanged checkout", result.stderr)
                self.assertEqual(list(self.staging.iterdir()), [])

    def test_all_stages_reject_staged_only_changes(self):
        module = self.source / "scripts/mechrevo_power.py"
        original = module.read_bytes()
        module.write_bytes(b"# changed index\n")
        self.git("add", "scripts/mechrevo_power.py")
        module.write_bytes(original)
        for name in ("prepare", "check", "package"):
            with self.subTest(name=name):
                result = self.phase(name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unchanged checkout", result.stderr)
                self.assertEqual(list(self.staging.iterdir()), [])

    def test_all_stages_reject_untracked_privileged_helper(self):
        self.git("rm", "--cached", "scripts/mechrevo-power-console-helper")
        self.git("commit", "-m", "Untrack helper")
        self.commit = self.git("rev-parse", "HEAD")
        self.env["MECHREVO_POWER_COMMIT"] = self.commit
        for name in ("prepare", "check", "package"):
            with self.subTest(name=name):
                result = self.phase(name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Required tracked source file missing", result.stderr)
                self.assertEqual(list(self.staging.iterdir()), [])

    def test_policy_contract_rejected_before_staging(self):
        relative = "console/resources/org.mechrevo.console.cpu-power.policy"
        policy = self.source / relative
        original = policy.read_text()
        changes = (
            ("/usr/lib/mechrevo-power-control/console-helper", "/usr/bin/python3"),
            (">--request</annotate>", ">set</annotate>"),
            ("<allow_any>no</allow_any>", "<allow_any>yes</allow_any>"),
            ("<allow_inactive>auth_admin</allow_inactive>", "<allow_inactive>yes</allow_inactive>"),
            ("<allow_active>auth_admin_keep</allow_active>", "<allow_active>yes</allow_active>"),
            ("</defaults>", "<allow_any>no</allow_any></defaults>"),
            ("</action>", '<annotate key="extra">yes</annotate></action>'),
            ("</policyconfig>", '<action id="extra"/></policyconfig>'),
            ("org.mechrevo.console.cpu-power", "org.mechrevo.console.oem"),
            ("</policyconfig>", ""),
        )
        for old, new in changes:
            with self.subTest(change=old):
                self.assertIn(old, original)
                policy.write_text(original.replace(old, new))
                self.commit_current()
                for name in ("check", "package"):
                    result = self.phase(name)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(list(self.staging.iterdir()), [])

    def test_real_fakeroot_accepts_clean_source_and_stages_fixed_helper_and_policy(self):
        result = self.phase("package", fakeroot=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for source in ("scripts/mechrevo-power-console-helper",
                       "console/resources/org.mechrevo.console.cpu-power.policy"):
            relative, content, mode = PAYLOAD[source]
            installed = self.staging / relative
            self.assertEqual(installed.read_bytes(), content)
            self.assertEqual(stat.S_IMODE(installed.stat().st_mode), mode)
            self.assertFalse(installed.is_symlink())

    def test_helper_uses_isolated_python_and_only_fixed_installed_module(self):
        launcher = self.path / "console-helper"
        text = (ROOT / "scripts/mechrevo-power-console-helper").read_text()
        self.assertTrue(text.startswith("#!/usr/bin/python3 -I\n"))
        module = self.path / "installed-controller.py"
        module.write_text(
            "import json,sys\n"
            "def console_main(argv=None):\n"
            " argv=sys.argv[1:] if argv is None else argv\n"
            " print(json.dumps({'isolated': bool(sys.flags.isolated), 'argv': argv}))\n"
            " return 0 if sys.flags.isolated and argv == ['--request', '{}'] else 77\n")
        fixed = "/usr/lib/mechrevo-power-control/mechrevo_power.py"
        self.assertIn(fixed, text)
        launcher.write_text(text.replace(fixed, str(module)))
        launcher.chmod(0o755)
        malicious = self.path / "injected"
        malicious.mkdir()
        marker = self.path / "unexpected-import"
        (malicious / "json.py").write_text(f"open({str(marker)!r}, 'w').write('imported')\n")
        (self.path / "mechrevo_power.py").write_text(f"open({str(marker)!r}, 'w').write('fallback')\n")
        environment = dict(self.env, PYTHONPATH=str(malicious))
        result = subprocess.run([str(launcher), "--request", "{}"], cwd=malicious,
                                env=environment, text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('"isolated": true', result.stdout)
        self.assertFalse(marker.exists())

    def install_wrapper(self, text):
        directory = self.path / "install-wrapper"
        directory.mkdir(exist_ok=True)
        wrapper = directory / "install"
        wrapper.write_text("#!/bin/bash\n" + text)
        wrapper.chmod(0o755)
        self.env["PATH"] = str(directory) + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin")

    def test_staging_rejects_post_install_helper_bytes_and_policy_modes(self):
        for suffix, mutate in (("console-helper", 'printf "changed" >> "${@: -1}"'),
                               ("org.mechrevo.console.cpu-power.policy", 'chmod 0777 "${@: -1}"')):
            with self.subTest(target=suffix):
                self.install_wrapper('/usr/bin/install "$@" || exit $?\n'
                                     f'case "${{@: -1}}" in */{suffix}) {mutate} ;; esac\n')
                result = self.phase("package")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("helper or policy bytes/modes", result.stderr)

    def test_coordinator_install_failure_does_not_stage_authorized_helper(self):
        self.install_wrapper('for arg in "$@"; do\n'
                             ' if [[ $arg == scripts/mechrevo_power.py ]]; then exit 73; fi\n'
                             'done\nexec /usr/bin/install "$@"\n')
        result = self.phase("package", errexit=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.staging / "usr/lib/mechrevo-power-control/console-helper").exists())
        self.assertFalse((self.staging / "usr/share/polkit-1/actions/org.mechrevo.console.cpu-power.policy").exists())

    def test_peak_trial_launcher_selects_checkout_or_installed_module_without_running_trial(self):
        launcher = ROOT / "scripts/mechrevo-peak-trial"
        code = compile(launcher.read_bytes(), str(launcher), "exec")
        for location, expected in (
                (self.source / "scripts/mechrevo-peak-trial",
                 self.source / "scripts/peak_limit_trial.py"),
                (self.path / "bin/mechrevo-peak-trial",
                 Path("/usr/lib/mechrevo-power-control/peak_limit_trial.py"))):
            with self.subTest(location=location), mock.patch("runpy.run_path") as run, \
                    mock.patch.object(sys, "path", list(sys.path)):
                exec(code, {"__file__": str(location)})
                run.assert_called_once_with(str(expected), run_name="__main__")

    def test_peak_trial_launches_real_modules_in_checkout_and_installed_layouts(self):
        launcher_text = (ROOT / "scripts/mechrevo-peak-trial").read_text()
        for layout in ("checkout", "installed"):
            with self.subTest(layout=layout):
                base = self.path / layout
                bin_dir = base / ("scripts" if layout == "checkout" else "usr/bin")
                module_dir = bin_dir if layout == "checkout" else base / "usr/lib/mechrevo-power-control"
                bin_dir.mkdir(parents=True)
                module_dir.mkdir(parents=True, exist_ok=True)
                launcher = bin_dir / "mechrevo-peak-trial"
                # Relocate only the absolute installed directory into the fake root.
                # The real runpy and sibling imports must execute in a fresh process.
                launcher.write_text(launcher_text.replace(
                    '"/usr/lib/mechrevo-power-control/peak_limit_trial.py"',
                    repr(str(module_dir / "peak_limit_trial.py"))))
                for filename in ("peak_limit_trial.py", "mechrevo_power.py", "mechrevo_thermal.py"):
                    shutil.copy2(ROOT / "scripts" / filename, module_dir / filename)
                result = subprocess.run([sys.executable, "-I", "-B", str(launcher), "--help"],
                                        cwd=self.path, text=True, capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("--peak-watts", result.stdout)
                self.assertIn("--recover", result.stdout)
                self.assertEqual(result.stderr, "")

    @unittest.skipIf(os.geteuid() == 0, "makepkg intentionally refuses root")
    def test_real_archive_is_frozen_and_contains_no_automatic_activation(self):
        # A branch change after the requested commit must not enter the package.
        for relative in PAYLOAD:
            (self.source / relative).write_bytes(b"# later unqualified change\n")
        (self.source / "foreign-file").write_text("not part of package\n")
        self.git("add", ".")
        self.git("commit", "-m", "Later fixture change")
        result = self.makepkg("--nodeps", "--noconfirm", "--nocolor")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        metadata = self.metadata()
        package_version = f"{metadata['pkgver'][0]}-{metadata['pkgrel'][0]}"
        package_arch = metadata["arch"][0]
        archives = list(self.build.glob(f"{PKGNAME}-{package_version}-{package_arch}.pkg.tar.*"))
        self.assertEqual(len(archives), 1, result.stdout + result.stderr)
        archive = archives[0]
        listing = subprocess.check_output(["bsdtar", "-tf", str(archive)], text=True).splitlines()
        files = {name for name in listing if not name.endswith("/")}
        self.assertEqual(files, {entry[0] for entry in PAYLOAD.values()} |
                         {".PKGINFO", ".BUILDINFO", ".MTREE"})
        destination = self.path / "unpacked"
        destination.mkdir()
        subprocess.run(["bsdtar", "-xf", str(archive), "-C", str(destination)], check=True,
                       capture_output=True)
        for _, (relative, content, mode) in PAYLOAD.items():
            target = destination / relative
            self.assertEqual(target.read_bytes(), content, relative)
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), mode, relative)
        package_info = (destination / ".PKGINFO").read_text()
        self.assertIn("depend = thermald=2:2.5.13\n", package_info)
        self.assertFalse((destination / ".INSTALL").exists())
        self.assertFalse((destination / "etc").exists())
        self.assertFalse((destination / "usr/lib/systemd").exists())
        self.assertFalse((destination / "usr/share/libalpm/hooks").exists())


if __name__ == "__main__":
    unittest.main()
