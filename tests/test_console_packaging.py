"""Check the console's first-time build contract without network or installation."""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
PACKAGING = ROOT / "packaging/arch/mechrevo-console"
REQUIRED_SOURCE = (
    "console/Cargo.toml",
    "console/Cargo.lock",
    "console/crates/cli/Cargo.toml",
    "console/crates/cli/src/main.rs",
    "console/crates/cli/src/oem_helper.rs",
    "console/crates/core/src/privilege.rs",
    "console/crates/gui/src/main.rs",
    "console/resources/org.mechrevo.console.oem.policy",
    "console/resources/org.mechrevo.touchpad.policy",
    "console/resources/99-mechrevo-touchpad.rules",
    "scripts/mechrevo_control.py",
    "scripts/mechrevo-gaming-hotkey.service",
    "scripts/mechrevo-ec-hotkey.service",
    "scripts/mechrevo-touchpad-sleep",
    "scripts/mechrevo_touchpad.py",
    "scripts/mechrevo-touchpad-restore@.service",
    "console/crates/gui/assets/fonts/Manrope-OFL.txt",
    "docs/MECHREVO-CONSOLE.md",
    "docs/PERFORMANCE-PROFILES.md",
    "LICENSE",
)


@unittest.skipUnless(shutil.which("git"), "Git is required for source fixtures")
class ConsolePackaging(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        parent.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix="console-packaging-", dir=parent)
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.source = self.path / "src/mechrevo-drivers"
        self.source.mkdir(parents=True)
        self.git("init", "-b", "main")
        for key, value in (("user.name", "Packaging test"),
                           ("user.email", "test@example.invalid"),
                           ("commit.gpgsign", "false")):
            self.git("config", key, value)
        for relative in REQUIRED_SOURCE:
            target = self.source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(f"Tracked source fixture: {relative}\n")
        for action, helper, inactive, active in (
                ("org.mechrevo.console.oem", "/usr/lib/mechrevo-console/oem-helper",
                 "auth_admin", "auth_admin_keep"),
                ("org.mechrevo.touchpad", "/usr/lib/mechrevo-control/touchpad-helper",
                 "no", "yes")):
            (self.source / f"console/resources/{action}.policy").write_text(
                f'<policyconfig><action id="{action}"><defaults>'
                f'<allow_any>no</allow_any><allow_inactive>{inactive}</allow_inactive>'
                f'<allow_active>{active}</allow_active></defaults>'
                '<annotate key="org.freedesktop.policykit.exec.path">'
                f'{helper}</annotate></action></policyconfig>\n'
            )
        self.git("add", ".")
        self.git("commit", "-m", "Pinned source fixture")
        self.commit = self.git("rev-parse", "HEAD")
        recipe, replacements = re.subn(
            r"(?m)^_source_commit=[0-9a-f]{40}$",
            f"_source_commit={self.commit}",
            (PACKAGING / "PKGBUILD").read_text(),
        )
        self.assertEqual(replacements, 1, "the package must pin one full commit")
        self.recipe = self.path / "PKGBUILD"
        self.recipe.write_text(recipe)
        self.calls = self.path / "cargo-calls.jsonl"
        commands = self.path / "bin"
        commands.mkdir()
        cargo = commands / "cargo"
        cargo.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "with open(os.environ['CONSOLE_TEST_CARGO_CALLS'], 'a') as output:\n"
            "    output.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            "sys.exit(int(os.environ.get('CONSOLE_TEST_CARGO_STATUS', '0')))\n"
        )
        cargo.chmod(0o755)
        self.env = dict(os.environ, CARCH="x86_64",
                        PATH=f"{commands}:{os.environ['PATH']}",
                        CONSOLE_TEST_CARGO_CALLS=str(self.calls))
        self.target = self.path / "cargo-target"
        self.env["CARGO_TARGET_DIR"] = str(self.target)
        release = self.target / "release"
        release.mkdir(parents=True)
        for binary in ("mechrevo-console", "mechrevo-console-gui",
                       "mechrevo-console-oem-helper"):
            target = release / binary
            target.write_text(f"#!/bin/sh\n# Fixture: {binary}\nexit 0\n")
            target.chmod(0o755)
        self.pkgdir = self.path / "pkg"
        self.pkgdir.mkdir()
        (self.source.parent / "best.lmm.mechrevo-console.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=MECHREVO Console\n"
            "Exec=mechrevo-console-gui\nTerminal=false\n"
        )

    def git(self, *args):
        result = subprocess.run(["git", "-C", str(self.source), *args],
                                text=True, capture_output=True, check=True)
        return result.stdout.strip()

    def run_phase(self, name):
        return subprocess.run(
            ["bash", "-e", "-c", 'source "$1"; srcdir="$2"; pkgdir="$3"; "$4"',
             "console-packaging-test", str(self.recipe), str(self.source.parent),
             str(self.pkgdir), name],
            env=self.env, text=True, capture_output=True,
        )

    def cargo_calls(self):
        if not self.calls.exists():
            return []
        return [json.loads(line) for line in self.calls.read_text().splitlines()]

    @unittest.skipUnless(shutil.which("makepkg"), "Arch metadata tool is required")
    def test_runtime_dependencies_include_bridge_and_direct_elf_runtime(self):
        result = subprocess.run(["makepkg", "--printsrcinfo"], cwd=PACKAGING,
                                text=True, capture_output=True, check=True)
        metadata = {}
        for line in result.stdout.splitlines():
            if line.strip():
                key, value = line.strip().split(" = ", 1)
                metadata.setdefault(key, []).append(value)
        self.assertEqual(metadata["pkgname"], ["mechrevo-console"])
        self.assertEqual(metadata["arch"], ["x86_64"])
        for dependency in ("python", "systemd", "polkit", "libgcc"):
            self.assertIn(dependency, metadata["depends"])
        self.assertFalse(any("dkms" in value for value in metadata["depends"]))
        self.assertNotIn("replaces", metadata)
        self.assertEqual(metadata["install"], ["mechrevo-console.install"])

    def test_every_non_git_source_has_a_matching_digest(self):
        # Read paired source/checksum arrays without interpreting shell in Python.
        result = subprocess.run(
            ["bash", "-c", 'source "$1"; for i in "${!source[@]}"; do '
             'printf "%s\\t%s\\n" "${source[$i]}" "${sha256sums[$i]}"; done',
             "test", str(self.recipe)], text=True, capture_output=True, check=True,
        )
        rows = [line.split("\t") for line in result.stdout.splitlines()]
        self.assertGreaterEqual(len(rows), 3)
        for source, checksum in rows:
            if "::git+" in source:
                self.assertEqual(checksum, "SKIP")
                self.assertIn(f"#commit={self.commit}", source)
            else:
                self.assertRegex(checksum, r"^[0-9a-f]{64}$")
                self.assertEqual(hashlib.sha256((PACKAGING / source).read_bytes()).hexdigest(),
                                 checksum)

    def test_prepare_populates_cache_with_locked_host_dependencies(self):
        result = self.run_phase("prepare")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.cargo_calls(), [[
            "fetch", "--manifest-path", "console/Cargo.toml", "--locked",
            "--target", "x86_64-unknown-linux-gnu",
        ]])

    def test_prepare_propagates_fetch_failure(self):
        self.env["CONSOLE_TEST_CARGO_STATUS"] = "42"
        result = self.run_phase("prepare")
        self.assertEqual(result.returncode, 42)
        self.assertEqual(len(self.cargo_calls()), 1)

    def test_prepare_rejects_modified_source_before_fetch(self):
        (self.source / "console/Cargo.lock").write_text("Changed dependency graph.\n")
        result = self.run_phase("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unchanged checkout", result.stderr)
        self.assertFalse(self.calls.exists())

    def test_prepare_rejects_wrong_commit_before_fetch(self):
        self.git("commit", "--allow-empty", "-m", "Unreviewed commit")
        result = self.run_phase("prepare")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unchanged checkout", result.stderr)
        self.assertFalse(self.calls.exists())

    def test_build_remains_locked_and_offline(self):
        result = self.run_phase("build")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = self.cargo_calls()
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][0], "build")
        for flag in ("--locked", "--offline", "--release", "--workspace", "--all-features"):
            self.assertIn(flag, calls[0])

    def test_package_installs_verified_touchpad_files_and_preserves_oem_helper(self):
        result = self.run_phase("package")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        expected = (
            ("scripts/mechrevo_touchpad.py",
             "usr/lib/mechrevo-control/touchpad-helper", 0o755),
            ("console/resources/org.mechrevo.touchpad.policy",
             "usr/share/polkit-1/actions/org.mechrevo.touchpad.policy", 0o644),
            ("console/resources/99-mechrevo-touchpad.rules",
             "usr/lib/udev/rules.d/99-mechrevo-touchpad.rules", 0o644),
            ("scripts/mechrevo-touchpad-restore@.service",
             "usr/lib/systemd/system/mechrevo-touchpad-restore@.service", 0o644),
            ("scripts/mechrevo-touchpad-sleep",
             "usr/lib/systemd/system-sleep/mechrevo-touchpad", 0o755),
            ("console/resources/org.mechrevo.console.oem.policy",
             "usr/share/polkit-1/actions/org.mechrevo.console.oem.policy", 0o644),
            ("console/crates/gui/assets/fonts/Manrope-OFL.txt",
             "usr/share/licenses/mechrevo-console/Manrope-OFL.txt", 0o644),
        )
        for source, installed, mode in expected:
            with self.subTest(installed=installed):
                target = self.pkgdir / installed
                self.assertFalse(target.is_symlink())
                self.assertEqual(target.read_bytes(), (self.source / source).read_bytes())
                self.assertEqual(target.stat().st_mode & 0o777, mode)
        oem_helper = self.pkgdir / "usr/lib/mechrevo-console/oem-helper"
        self.assertEqual(oem_helper.read_bytes(),
                         (self.target / "release/mechrevo-console-oem-helper").read_bytes())
        self.assertEqual(oem_helper.stat().st_mode & 0o777, 0o755)
        self.assertFalse((self.pkgdir / "usr/share/licenses/mechrevo-console/Cormorant-OFL.txt").exists())
        self.assertFalse((self.pkgdir / "usr/lib/systemd/system/mechrevo-touchpad-restore.service").exists())

    def test_package_rejects_symlink_touchpad_helper(self):
        helper = self.source / "scripts/mechrevo_touchpad.py"
        helper.unlink()
        helper.symlink_to("mechrevo_control.py")
        result = self.run_phase("package")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Required regular touchpad source missing", result.stderr)
        self.assertFalse((self.pkgdir / "usr/lib/mechrevo-control/touchpad-helper").exists())

    @unittest.skipUnless(shutil.which("xmllint") and shutil.which("desktop-file-validate"),
                         "XML and desktop validators are required")
    def test_check_rejects_touchpad_policy_privilege_or_path_drift(self):
        policy = self.source / "console/resources/org.mechrevo.touchpad.policy"
        original = policy.read_text()
        result = self.run_phase("check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for old, new in (
                ("<allow_any>no</allow_any>", "<allow_any>yes</allow_any>"),
                ("<allow_any>no</allow_any>",
                 "<allow_any>no</allow_any><allow_any>yes</allow_any>"),
                ("<allow_inactive>no</allow_inactive>", "<allow_inactive>yes</allow_inactive>"),
                ("/usr/lib/mechrevo-control/touchpad-helper", "/usr/bin/python3")):
            with self.subTest(changed=old):
                policy.write_text(original.replace(old, new))
                result = self.run_phase("check")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Touchpad policy", result.stderr)
        policy.write_text(original)


if __name__ == "__main__":
    unittest.main()
