"""Local-only regression tests: real Git repos, mocked GitHub PR API, no network."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("sync_upstream", ROOT / "scripts/sync_upstream.py")
SYNC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SYNC)


def source(brands=("TUXEDO", "MECHREVO")):
    entries = "".join(
        f'\t{{ .matches = {{ DMI_MATCH({field}, "{brand}"), }} }},\n'
        for brand in brands for field in SYNC.FIELDS
    )
    return ("static const struct dmi_system_id tuxedo_dmi_string_match[] = {\n"
            + entries + "\t{ }\n};\n"
            + "bool tuxedo_is_compatible(void) { return dmi_check_system(tuxedo_dmi_string_match); }\n")


class Guards(unittest.TestCase):
    def test_numeric_stable_selection(self):
        refs = ["refs/tags/upstream/v4.9.0", "refs/tags/upstream/v4.24.0",
                "refs/tags/upstream/v5.0.0_rc", "refs/tags/v99.0.0"]
        self.assertEqual(SYNC.latest_release(refs), "refs/tags/upstream/v4.24.0")

    def test_no_stable_release(self):
        with self.assertRaises(SYNC.SyncError):
            SYNC.latest_release(["refs/tags/upstream/v4.25.0_rc"])

    def test_real_downstream_source(self):
        SYNC.validate_mechrevo((ROOT / SYNC.COMPAT_PATH).read_text())

    def test_comments_do_not_satisfy_guard(self):
        with self.assertRaises(SYNC.SyncError):
            SYNC.validate_mechrevo(source(("TUXEDO",)) + "/* " + source() + " */")

    def test_matches_must_be_independent(self):
        broken = source().replace(
            'DMI_MATCH(DMI_SYS_VENDOR, "MECHREVO"),',
            'DMI_MATCH(DMI_SYS_VENDOR, "MECHREVO"), DMI_MATCH(DMI_BOARD_VENDOR, "OTHER"),',
        )
        with self.assertRaises(SYNC.SyncError):
            SYNC.validate_mechrevo(broken)

    def test_removed_table_use_is_rejected(self):
        with self.assertRaises(SYNC.SyncError):
            SYNC.validate_mechrevo(source().replace("dmi_check_system", "different_check"))


class GitIntegration(unittest.TestCase):
    def setUp(self):
        parent = ROOT / ".local-tests"
        parent.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.upstream = self.path / "upstream"
        self.origin = self.path / "origin.git"
        self.work = self.path / "maintenance"
        self.cmd(self.path, "git", "init", "--bare", str(self.origin))
        self.cmd(self.path, "git", "init", "-b", "main", str(self.upstream))
        self.configure(self.upstream)
        (self.upstream / SYNC.COMPAT_PATH).parent.mkdir(parents=True)
        (self.upstream / SYNC.COMPAT_PATH).write_text(source(("TUXEDO",)))
        (self.upstream / ".gitignore").write_text("/.sync-worktrees/\n")
        self.commit(self.upstream, "Initial upstream")
        self.g(self.upstream, "tag", "v1.0.0")
        self.cmd(self.path, "git", "clone", str(self.upstream), str(self.work))
        self.configure(self.work)
        self.g(self.work, "remote", "rename", "origin", "tuxedo")
        self.g(self.work, "remote", "add", "origin", str(self.origin))
        (self.work / SYNC.COMPAT_PATH).write_text(source())
        self.commit(self.work, "Enable MECHREVO recognition")
        self.g(self.work, "push", "origin", "main")
        self.base = self.g(self.work, "rev-parse", "HEAD")
        self.pr = mock.patch.object(SYNC, "ensure_pr")
        self.ensure_pr = self.pr.start()
        self.addCleanup(self.pr.stop)

    def cmd(self, cwd, *args):
        result = subprocess.run(args, cwd=cwd, text=True, capture_output=True)
        if result.returncode:
            self.fail(f"{' '.join(args)}\n{result.stdout}{result.stderr}")
        return result.stdout.strip()

    def g(self, cwd, *args):
        return self.cmd(cwd, "git", *args)

    def configure(self, root):
        for key, value in (("user.name", "Local test"), ("user.email", "test@example.invalid"),
                           ("commit.gpgsign", "false"), ("tag.gpgsign", "false")):
            self.g(root, "config", key, value)

    def commit(self, root, message):
        self.g(root, "add", ".")
        self.g(root, "commit", "-m", message)

    def release(self):
        (self.upstream / "stable.txt").write_text("New stable release\n")
        self.commit(self.upstream, "Stable changes")
        self.g(self.upstream, "tag", "v1.1.0")
        return self.g(self.upstream, "rev-parse", "HEAD")

    def sync(self, dry_run=False):
        SYNC.sync(self.work, "example/mechrevo-drivers", str(self.upstream), dry_run)

    def ref(self, name):
        return self.g(self.origin, "rev-parse", name)

    def test_noop_mirrors_without_pr(self):
        self.sync()
        self.assertEqual(self.ref("refs/heads/main"), self.base)
        self.assertEqual(self.ref("refs/heads/upstream"), self.g(self.upstream, "rev-parse", "main"))
        self.ensure_pr.assert_not_called()

    def test_release_pr_preserves_patch_and_excludes_unreleased(self):
        release = self.release()
        (self.upstream / "unreleased.txt").write_text("Not a stable release\n")
        self.commit(self.upstream, "Unreleased development")
        self.g(self.upstream, "tag", "v2.0.0_rc")
        self.sync()
        bot = f"refs/heads/{SYNC.PR_BRANCH}"
        candidate = self.ref(bot)
        self.assertEqual(self.ref("refs/heads/main"), self.base)
        self.assertEqual(self.ref("refs/heads/upstream"), self.g(self.upstream, "rev-parse", "main"))
        self.assertEqual(self.g(self.origin, "show", "-s", "--format=%P", bot), f"{self.base} {release}")
        self.assertNotIn("unreleased.txt", self.g(self.origin, "ls-tree", "--name-only", bot))
        SYNC.validate_mechrevo(self.g(self.origin, "show", f"{bot}:{SYNC.COMPAT_PATH}"))
        self.ensure_pr.assert_called_once()
        self.sync()
        self.assertEqual(self.ref(bot), candidate)
        # A new maintenance commit is included in the regenerated PR, not lost.
        (self.work / "maintainer.txt").write_text("Keep this change\n")
        self.commit(self.work, "Maintainer change")
        self.g(self.work, "push", "origin", "main")
        self.sync()
        self.assertIn("maintainer.txt", self.g(self.origin, "ls-tree", "--name-only", bot))

    def test_dry_run_does_not_publish(self):
        self.release()
        before = self.g(self.origin, "show-ref")
        self.sync(dry_run=True)
        self.assertEqual(self.g(self.origin, "show-ref"), before)
        self.ensure_pr.assert_not_called()

    def test_conflict_stops_without_publishing_candidate(self):
        (self.upstream / SYNC.COMPAT_PATH).write_text("Upstream replaced this interface\n")
        self.commit(self.upstream, "Incompatible upstream refactor")
        self.g(self.upstream, "tag", "v1.1.0")
        with self.assertRaises(SYNC.SyncError):
            self.sync()
        self.assertEqual(self.ref("refs/heads/main"), self.base)
        self.assertNotIn(SYNC.PR_BRANCH, self.g(self.origin, "show-ref"))
        self.assertEqual(self.g(self.work, "worktree", "list", "--porcelain").count("worktree "), 1)
        self.ensure_pr.assert_not_called()

    def test_missing_main_patch_fails_closed(self):
        (self.work / SYNC.COMPAT_PATH).write_text(source(("TUXEDO",)))
        self.commit(self.work, "Accidentally drop patch")
        self.g(self.work, "push", "origin", "main")
        before = self.g(self.origin, "show-ref")
        with self.assertRaises(SYNC.SyncError):
            self.sync()
        self.assertEqual(self.g(self.origin, "show-ref"), before)

    def test_rewritten_upstream_mirror_is_not_forced(self):
        self.release()
        self.sync()
        mirror = self.ref("refs/heads/upstream")
        self.g(self.upstream, "reset", "--hard", "v1.0.0")
        with self.assertRaisesRegex(SYNC.SyncError, "rewritten"):
            self.sync()
        self.assertEqual(self.ref("refs/heads/upstream"), mirror)

    def test_rewritten_upstream_tag_is_rejected(self):
        self.sync()
        self.release()
        self.g(self.upstream, "tag", "-f", "v1.0.0")
        before = self.g(self.origin, "show-ref")
        with self.assertRaises(SYNC.SyncError):
            self.sync()
        self.assertEqual(self.g(self.origin, "show-ref"), before)

    def test_release_not_on_upstream_main_is_rejected(self):
        self.g(self.upstream, "checkout", "--orphan", "other-release-line")
        (self.upstream / "orphan.txt").write_text("Different release history\n")
        self.commit(self.upstream, "Unrelated release")
        self.g(self.upstream, "tag", "v99.0.0")
        self.g(self.upstream, "checkout", "main")
        before = self.g(self.origin, "show-ref")
        with self.assertRaisesRegex(SYNC.SyncError, "not on upstream main"):
            self.sync()
        self.assertEqual(self.g(self.origin, "show-ref"), before)

    def test_main_race_stops_candidate_publication(self):
        self.release()
        (self.work / "new-main.txt").write_text("Concurrent maintainer change\n")
        self.commit(self.work, "Future main")
        newer = self.g(self.work, "rev-parse", "HEAD")
        real_git = SYNC.git

        def race(root, *args, **kwargs):
            if args[:3] == ("ls-remote", "origin", "refs/heads/main"):
                self.g(self.work, "push", "origin", "main")
            return real_git(root, *args, **kwargs)

        with mock.patch.object(SYNC, "git", side_effect=race):
            with self.assertRaisesRegex(SYNC.SyncError, "main changed"):
                self.sync()
        self.assertEqual(self.ref("refs/heads/main"), newer)
        self.assertNotIn(SYNC.PR_BRANCH, self.g(self.origin, "show-ref"))
        self.ensure_pr.assert_not_called()

    def test_exact_branch_lease_rejects_concurrent_writer(self):
        self.release()
        real_git = SYNC.git

        def race(root, *args, **kwargs):
            if args[0] == "push" and any(arg.startswith("--force-with-lease=") for arg in args):
                self.g(self.origin, "update-ref", f"refs/heads/{SYNC.PR_BRANCH}", self.base)
            return real_git(root, *args, **kwargs)

        with mock.patch.object(SYNC, "git", side_effect=race):
            with self.assertRaises(SYNC.SyncError):
                self.sync()
        self.assertEqual(self.ref(f"refs/heads/{SYNC.PR_BRANCH}"), self.base)
        self.assertEqual(self.ref("refs/heads/main"), self.base)
        self.ensure_pr.assert_not_called()

    def test_dirty_worktree_is_rejected(self):
        (self.work / "untracked.txt").write_text("Do not consume local files\n")
        with self.assertRaisesRegex(SYNC.SyncError, "clean"):
            self.sync()


class PullRequests(unittest.TestCase):
    def result(self, text):
        return subprocess.CompletedProcess([], 0, stdout=text, stderr="")

    def test_create_once(self):
        with mock.patch.object(SYNC, "run", side_effect=[self.result("[]"), self.result("")]) as run:
            SYNC.ensure_pr(ROOT, "owner/repo", "v1.2.0", "release-sha", "base-sha")
        self.assertEqual(run.call_args_list[-1].args[:3], ("gh", "pr", "create"))

    def test_update_existing_and_skip_unchanged(self):
        with mock.patch.object(SYNC, "run", side_effect=[self.result('[{"number": 7}]'),
                         self.result('{"title":"old", "body":"old"}'), self.result("")]) as run:
            SYNC.ensure_pr(ROOT, "owner/repo", "v1.2.0", "release-sha", "base-sha")
        args = run.call_args_list[-1].args
        self.assertEqual(args[:4], ("gh", "pr", "edit", "7"))
        import json
        current = {"title": args[args.index("--title") + 1], "body": args[args.index("--body") + 1]}
        with mock.patch.object(SYNC, "run", side_effect=[self.result('[{"number": 7}]'),
                         self.result(json.dumps(current))]) as run:
            SYNC.ensure_pr(ROOT, "owner/repo", "v1.2.0", "release-sha", "base-sha")
        self.assertEqual(run.call_count, 2)


if __name__ == "__main__":
    unittest.main()
