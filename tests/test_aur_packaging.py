"""Local AUR packaging checks; no network, system installation or module loading."""

import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PACKAGING = ROOT / "packaging/aur/mechrevo-drivers-dkms-git"
PKGBUILD = PACKAGING / "PKGBUILD"
PKGNAME = "mechrevo-drivers-dkms-git"


class AurPackaging(unittest.TestCase):
    def setUp(self):
        parent = ROOT / ".local-tests"
        parent.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)

    def bash(self, script, *args):
        result = subprocess.run(
            ["bash", "-e", "-c", script, "packaging-test", str(PKGBUILD), *map(str, args)],
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout.strip()

    def git(self, source, *args):
        result = subprocess.run(
            ["git", "-C", str(source), *args], text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout.strip()

    def source_repo(self):
        source = self.path / "mechrevo-drivers"
        source.mkdir()
        self.git(source, "init", "-b", "main")
        for key, value in (("user.name", "Packaging test"),
                           ("user.email", "test@example.invalid"),
                           ("commit.gpgsign", "false")):
            self.git(source, "config", key, value)
        (source / "fixture").write_text("initial\n")
        self.git(source, "add", "fixture")
        self.git(source, "commit", "-m", "Initial fixture")
        return source

    def version(self):
        return self.bash('source "$1"; srcdir="$2"; pkgver', self.path)

    def test_metadata_and_conflicts(self):
        result = subprocess.run(
            ["makepkg", "--printsrcinfo"], cwd=PACKAGING, text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata = {}
        for line in result.stdout.splitlines():
            if not line.strip():
                continue
            key, value = line.strip().split(" = ", 1)
            metadata.setdefault(key, []).append(value)
        self.assertEqual(metadata["pkgbase"], [PKGNAME])
        self.assertEqual(metadata["pkgname"], [PKGNAME])
        self.assertEqual(metadata["arch"], ["x86_64"])
        self.assertEqual(metadata["depends"], ["dkms"])
        self.assertEqual(metadata["makedepends"], ["git"])
        self.assertEqual(metadata["license"], ["GPL-2.0-or-later"])
        self.assertEqual(metadata["source"], [
            "mechrevo-drivers::git+https://github.com/LIghtJUNction/mechrevo-drivers.git#branch=main",
            "dkms.conf",
        ])
        for name in ("mechrevo-drivers-dkms", "tuxedo-drivers-dkms",
                     "tuxedo-drivers-dkms-git", "tuxedo-keyboard-dkms",
                     "tuxedo-keyboard-ite-dkms"):
            self.assertIn(name, metadata["conflicts"])
        self.assertNotIn("replaces", metadata)
        self.assertIn("mechrevo-drivers-dkms=" + metadata["pkgver"][0], metadata["provides"])
        self.assertIn("tuxedo-drivers-dkms=" + metadata["pkgver"][0], metadata["provides"])
        self.assertEqual(metadata["sha256sums"], [
            "SKIP", hashlib.sha256((PACKAGING / "dkms.conf").read_bytes()).hexdigest(),
        ])

    def test_dkms_matches_all_kbuild_modules(self):
        expected = {}
        for kbuild in (ROOT / "src").rglob("Kbuild"):
            directory = kbuild.parent.relative_to(ROOT / "src").as_posix()
            for name in re.findall(r"^obj-m\s*(?:\+=|:=|=)\s*(\w+)\.o\s*$",
                                   kbuild.read_text(), re.MULTILINE):
                expected[name] = directory
        output = self.bash('''
            kernel_source_dir=/headers/selected-kernel
            dkms_tree=/sandbox/dkms
            source "$2"
            printf '%s\\n' "$PACKAGE_NAME" "$PACKAGE_VERSION" "$AUTOINSTALL" "${MAKE[0]}"
            for i in "${!BUILT_MODULE_NAME[@]}"; do
                printf '%s|%s|%s|%s\\n' "$i" "${BUILT_MODULE_NAME[$i]}" \\
                    "${BUILT_MODULE_LOCATION[$i]:-.}" "${DEST_MODULE_LOCATION[$i]}"
            done
        ''', PACKAGING / "dkms.conf").splitlines()
        self.assertEqual(output[:3], ["mechrevo-drivers", "@PKGVER@", "yes"])
        self.assertEqual(output[3], 'make -C "/headers/selected-kernel" '
                         'M="/sandbox/dkms/mechrevo-drivers/@PKGVER@/build" modules')
        actual = {}
        for index, row in enumerate(output[4:]):
            number, name, directory, destination = row.split("|")
            self.assertEqual(int(number), index)
            self.assertTrue(destination.startswith("/kernel/"))
            self.assertNotIn(name, actual)
            actual[name] = directory.rstrip("/")
        self.assertEqual(len(actual), 27)
        self.assertEqual(actual, expected)

    def test_version_uses_only_reachable_final_tags(self):
        source = self.source_repo()
        self.git(source, "tag", "v4.24.0")
        (source / "fixture").write_text("maintenance\n")
        self.git(source, "commit", "-am", "Maintenance change")
        self.git(source, "tag", "v4.25.0_rc")
        self.git(source, "tag", "v5.0.0-beta")
        self.git(source, "switch", "-c", "future")
        (source / "fixture").write_text("unmerged release\n")
        self.git(source, "commit", "-am", "Future release")
        self.git(source, "tag", "v99.0.0")
        self.git(source, "switch", "main")
        self.assertEqual(self.version(), "4.24.0.r1.g" + self.git(source, "rev-parse", "--short", "HEAD"))

    def test_version_without_tags(self):
        source = self.source_repo()
        self.assertEqual(self.version(), "r1.g" + self.git(source, "rev-parse", "--short", "HEAD"))

    def test_staged_package_layout_and_permissions(self):
        source = self.path / "mechrevo-drivers"
        tracked = subprocess.check_output(
            ["git", "ls-files", "src", "files", "LICENSE", "README.md", "docs"],
            cwd=ROOT, text=True,
        ).splitlines()
        for relative in tracked:
            destination = source / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, destination)
        shutil.copy2(PACKAGING / "dkms.conf", self.path / "dkms.conf")
        pkgdir = self.path / "pkg"
        version = "4.24.0.r1.g0123456"
        self.bash('source "$1"; srcdir="$2"; pkgdir="$3"; pkgver="$4"; package',
                  self.path, pkgdir, version)
        dkmsdir = pkgdir / "usr/src" / ("mechrevo-drivers-" + version)
        config = (dkmsdir / "dkms.conf").read_text()
        self.assertIn('PACKAGE_VERSION="' + version + '"', config)
        self.assertNotIn("@PKGVER@", config)
        self.assertTrue((dkmsdir / "Kbuild").is_file())
        self.assertFalse((dkmsdir / "src").exists())
        self.assertFalse(list(pkgdir.rglob("*.ko")))
        for relative in tracked:
            if relative.startswith("src/"):
                installed = dkmsdir / relative.removeprefix("src/")
            elif relative.startswith("files/"):
                installed = pkgdir / relative.removeprefix("files/")
            elif relative == "LICENSE":
                installed = pkgdir / "usr/share/licenses" / PKGNAME / "LICENSE"
            else:
                installed = pkgdir / "usr/share/doc" / PKGNAME / relative
            self.assertEqual(installed.read_bytes(), (ROOT / relative).read_bytes(), relative)
            self.assertEqual(installed.stat().st_mode & 0o777, 0o644, relative)
        self.assertEqual((dkmsdir / "dkms.conf").stat().st_mode & 0o777, 0o644)


if __name__ == "__main__":
    unittest.main()
