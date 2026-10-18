"""Run the real console package functions against isolated, committed fixtures.

These tests stage inert executable fixtures; they never run a helper or polkit.
"""
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
RECIPE = ROOT / "packaging/arch/mechrevo-console"
SOURCE_FILES = (
    "console/Cargo.toml", "console/Cargo.lock",
    "console/crates/cli/Cargo.toml", "console/crates/cli/src/main.rs",
    "console/crates/cli/src/oem_helper.rs", "console/crates/cli/src/fan_helper.rs",
    "console/crates/core/src/privilege.rs", "console/crates/core/src/fan_curve.rs",
    "console/crates/core/src/fan_curve/linux.rs",
    "console/crates/core/src/fan_curve/broker.rs", "console/crates/core/src/fan_curve/client.rs",
    "console/crates/gui/src/main.rs",
    "console/resources/org.mechrevo.console.oem.policy",
    "console/resources/org.mechrevo.console.fan.policy",
    "console/crates/gui/assets/fonts/Cormorant-OFL.txt",
    "console/crates/gui/assets/fonts/Manrope-OFL.txt",
    "docs/MECHREVO-CONSOLE.md", "LICENSE",
)
BINARIES = ("mechrevo-console", "mechrevo-console-gui",
            "mechrevo-console-oem-helper", "mechrevo-console-fan-helper")


class ConsoleFanPackagingTests(unittest.TestCase):
    def setUp(self):
        base = Path(os.environ.get("MECHREVO_TEST_TMPDIR", ROOT / ".local-tests"))
        self.temp = tempfile.TemporaryDirectory(prefix="fan-packaging-", dir=base)
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.src = self.base / "src"
        self.source = self.src / "mechrevo-drivers"
        self.source.mkdir(parents=True)
        self.pkg = self.base / "pkg"
        self.pkg.mkdir()
        self.target = self.base / "target"
        (self.target / "release").mkdir(parents=True)
        self.env = os.environ.copy()
        for variable in ("GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
                         "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"):
            self.env.pop(variable, None)
        self.env.update({"CARGO_TARGET_DIR": str(self.target), "LC_ALL": "C"})
        for relative in SOURCE_FILES:
            path = self.source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.suffix == ".policy":
                shutil.copyfile(ROOT / relative, path)
            else:
                path.write_text("inert source fixture\n")
        shutil.copyfile(RECIPE / "best.lmm.mechrevo-console.desktop",
                        self.src / "best.lmm.mechrevo-console.desktop")
        for name in BINARIES:
            path = self.target / "release" / name
            path.write_bytes(b"#!/bin/sh\nexit 77\n# inert " + name.encode() + b"\n")
            path.chmod(0o755)
        self.git("init", "--quiet")
        self.commit()

    def git(self, *args):
        return subprocess.run(
            ["git", "-c", "user.name=Packaging Fixture", "-c",
             "user.email=packaging-fixture@example.invalid", "-c",
             "core.hooksPath=/dev/null", "-c", "commit.gpgSign=false", *args],
            cwd=self.source, env=self.env, text=True, capture_output=True,
            check=True).stdout.strip()

    def commit(self):
        self.git("add", "--all")
        self.git("commit", "--quiet", "--allow-empty", "-m", "fixture")
        self.commit_id = self.git("rev-parse", "HEAD")

    def invoke(self, function):
        return subprocess.run(
            ["bash", "-c", 'source "$1"; srcdir=$2; pkgdir=$3; '
             '_source_commit=$4; "$5"', "package-contract",
             str(RECIPE / "PKGBUILD"), str(self.src), str(self.pkg),
             self.commit_id, function],
            cwd=self.base, env=self.env, capture_output=True, text=True)

    def assert_rejected(self, function="package", message=None):
        result = self.invoke(function)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        if message:
            self.assertIn(message, result.stdout + result.stderr)
        self.assertEqual(list(self.pkg.iterdir()), [], "rejection must precede staging")
        return result

    def mutate_policy(self, old, new, role="fan"):
        policy = self.source / f"console/resources/org.mechrevo.console.{role}.policy"
        original = policy.read_text()
        self.assertIn(old, original)
        policy.write_text(original.replace(old, new))
        self.commit()

    def install_wrapper(self, script):
        tools = self.base / "tools"
        tools.mkdir()
        wrapper = tools / "install"
        wrapper.write_text("#!/bin/bash\n/usr/bin/install \"$@\" || exit $?\n" + script)
        wrapper.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]

    def test_real_prepare_check_and_package_install_independent_helpers(self):
        for function in ("prepare", "check", "package"):
            result = self.invoke(function)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for role in ("oem", "fan"):
            with self.subTest(role=role):
                helper = self.pkg / f"usr/lib/mechrevo-console/{role}-helper"
                binary = self.target / f"release/mechrevo-console-{role}-helper"
                policy = self.pkg / f"usr/share/polkit-1/actions/org.mechrevo.console.{role}.policy"
                self.assertEqual(helper.read_bytes(), binary.read_bytes())
                self.assertEqual(stat.S_IMODE(helper.stat().st_mode), 0o755)
                self.assertEqual(policy.read_bytes(), (self.source / "console/resources" / policy.name).read_bytes())
                self.assertEqual(stat.S_IMODE(policy.stat().st_mode), 0o644)
                self.assertFalse(helper.is_symlink())
                self.assertFalse(policy.is_symlink())
        self.assertFalse((self.pkg / "usr/bin/mechrevo-console-fan-helper").exists())
        self.assertFalse((self.pkg / "usr/lib/systemd").exists())
        self.assertFalse((self.pkg / ".INSTALL").exists())

    def test_each_phase_rejects_untracked_fan_source(self):
        self.git("rm", "--cached", "console/crates/cli/src/fan_helper.rs")
        self.git("commit", "--quiet", "-m", "untrack helper")
        self.commit_id = self.git("rev-parse", "HEAD")
        for function in ("prepare", "check", "package"):
            with self.subTest(function=function):
                self.assert_rejected(function, "Required tracked regular source file missing")

    def test_each_phase_rejects_missing_fan_policy(self):
        (self.source / "console/resources/org.mechrevo.console.fan.policy").unlink()
        self.commit()
        for function in ("prepare", "check", "package"):
            with self.subTest(function=function):
                self.assert_rejected(function, "org.mechrevo.console.fan.policy")

    def test_each_phase_rejects_untracked_fan_policy(self):
        self.git("rm", "--cached", "console/resources/org.mechrevo.console.fan.policy")
        self.git("commit", "--quiet", "-m", "untrack policy")
        self.commit_id = self.git("rev-parse", "HEAD")
        for function in ("prepare", "check", "package"):
            with self.subTest(function=function):
                self.assert_rejected(function, "Required tracked regular source file missing")

    def test_committed_policy_symlink_is_rejected(self):
        policy = self.source / "console/resources/org.mechrevo.console.fan.policy"
        real = self.base / "real.policy"
        policy.rename(real)
        policy.symlink_to(real)
        self.commit()
        self.assert_rejected(message="Required tracked regular source file missing")

    def test_committed_helper_source_symlink_is_rejected(self):
        source = self.source / "console/crates/cli/src/fan_helper.rs"
        real = self.base / "real.rs"
        source.rename(real)
        source.symlink_to(real)
        self.commit()
        self.assert_rejected(message="Required tracked regular source file missing")

    def test_dirty_reviewed_source_is_rejected(self):
        (self.source / "console/crates/cli/src/fan_helper.rs").write_text("unreviewed\n")
        self.assert_rejected(message="unchanged checkout")

    def test_mismatched_source_pin_is_rejected(self):
        self.commit_id = "0" * 40
        self.assert_rejected(message="pinned console commit")

    def test_missing_helper_binary_is_rejected_before_staging(self):
        (self.target / "release/mechrevo-console-fan-helper").unlink()
        self.assert_rejected(message="Required regular executable missing")

    def test_nonexecutable_helper_binary_is_rejected(self):
        (self.target / "release/mechrevo-console-fan-helper").chmod(0o644)
        self.assert_rejected(message="Required regular executable missing")

    def test_helper_binary_symlink_is_rejected(self):
        helper = self.target / "release/mechrevo-console-fan-helper"
        real = self.base / "real-helper"
        helper.rename(real)
        helper.symlink_to(real)
        self.assert_rejected(message="Required regular executable missing")

    def test_wrong_fan_action_is_rejected(self):
        self.mutate_policy('id="org.mechrevo.console.fan"', 'id="org.mechrevo.console.oem"')
        self.assert_rejected(message="fan policy action")

    def test_wrong_fan_exec_path_is_rejected(self):
        self.mutate_policy("/usr/lib/mechrevo-console/fan-helper", "/usr/bin/bash")
        self.assert_rejected(message="fan policy action")

    def test_wrong_serve_argument_is_rejected(self):
        self.mutate_policy(">--serve</annotate>", ">--write-anything</annotate>")
        self.assert_rejected(message="fan policy action")

    def test_relaxed_authorization_is_rejected(self):
        self.mutate_policy("<allow_active>auth_admin_keep</allow_active>",
                           "<allow_active>yes</allow_active>")
        self.assert_rejected(message="fan policy action")

    def test_relaxed_inactive_authorization_is_rejected(self):
        self.mutate_policy("<allow_inactive>auth_admin</allow_inactive>",
                           "<allow_inactive>yes</allow_inactive>")
        self.assert_rejected(message="fan policy action")

    def test_relaxed_nonlocal_authorization_is_rejected(self):
        self.mutate_policy("<allow_any>no</allow_any>", "<allow_any>yes</allow_any>")
        self.assert_rejected(message="fan policy action")

    def test_duplicate_allow_active_is_rejected_even_with_correct_first_value(self):
        self.mutate_policy("<allow_active>auth_admin_keep</allow_active>",
                           "<allow_active>auth_admin_keep</allow_active><allow_active>yes</allow_active>")
        self.assert_rejected(message="fan policy action")

    def test_duplicate_action_is_rejected(self):
        self.mutate_policy("</policyconfig>", '<action id="org.mechrevo.console.other"/></policyconfig>')
        self.assert_rejected(message="fan policy action")

    def test_extra_gui_authorization_annotation_is_rejected(self):
        self.mutate_policy("</action>", '<annotate key="org.freedesktop.policykit.exec.allow_gui">true</annotate></action>')
        self.assert_rejected(message="fan policy action")

    def test_malformed_policy_is_rejected_by_real_xml_parser(self):
        self.mutate_policy("</policyconfig>", "<policyconfig>")
        self.assert_rejected()

    def test_oem_path_contract_is_not_weakened(self):
        self.mutate_policy("/usr/lib/mechrevo-console/oem-helper", "/usr/bin/sh", role="oem")
        self.assert_rejected(message="oem policy action")

    def test_post_install_helper_mode_drift_is_rejected(self):
        self.install_wrapper('target=${!#}\n[[ $target != */fan-helper ]] || chmod 0777 "$target"\n')
        result = self.invoke("package")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Installed fan helper or policy bytes/modes", result.stderr)

    def test_post_install_policy_bytes_drift_is_rejected(self):
        self.install_wrapper('target=${!#}\n[[ $target != */org.mechrevo.console.fan.policy ]] || printf "changed" >> "$target"\n')
        result = self.invoke("package")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Installed fan helper or policy bytes/modes", result.stderr)

    def test_post_install_policy_mode_drift_is_rejected(self):
        self.install_wrapper('target=${!#}\n[[ $target != */org.mechrevo.console.fan.policy ]] || chmod 0666 "$target"\n')
        result = self.invoke("package")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Installed fan helper or policy bytes/modes", result.stderr)

    def test_post_install_policy_symlink_is_rejected(self):
        self.install_wrapper('target=${!#}\nif [[ $target == */org.mechrevo.console.fan.policy ]]; then\n'
                             '  mv "$target" "$target.real"\n  ln -s "$target.real" "$target"\nfi\n')
        result = self.invoke("package")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Installed fan helper or policy bytes/modes", result.stderr)


if __name__ == "__main__":
    unittest.main()
