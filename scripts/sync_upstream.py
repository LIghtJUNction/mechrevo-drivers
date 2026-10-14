#!/usr/bin/env python3
"""Mirror TUXEDO main and propose stable-release merges without rewriting main."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

UPSTREAM_URL = "https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers.git"
PR_BRANCH = "automation/sync-upstream"
COMPAT_PATH = "src/tuxedo_compatibility_check/tuxedo_compatibility_check.c"
FIELDS = ("DMI_SYS_VENDOR", "DMI_BOARD_VENDOR", "DMI_CHASSIS_VENDOR")


class SyncError(RuntimeError):
    pass


def run(*args, cwd=None, check=True):
    result = subprocess.run(args, cwd=cwd, text=True, capture_output=True)
    if check and result.returncode:
        raise SyncError(f"{' '.join(args)}\n{result.stdout}{result.stderr}")
    return result


def git(root, *args, check=True):
    return run("git", *args, cwd=root, check=check)


def ancestor(root, older, newer):
    result = git(root, "merge-base", "--is-ancestor", older, newer, check=False)
    if result.returncode not in (0, 1):
        raise SyncError(result.stderr)
    return result.returncode == 0


def latest_release(refs):
    """Ignore RCs, betas and fork tags; sort numeric components, not text."""
    candidates = []
    for ref in refs:
        match = re.fullmatch(r"refs/tags/upstream/v(\d+)\.(\d+)\.(\d+)", ref)
        if match:
            candidates.append((tuple(map(int, match.groups())), ref))
    if not candidates:
        raise SyncError("No stable upstream vX.Y.Z release found")
    return max(candidates)[1]


def validate_mechrevo(source):
    """A source guard, not a substitute for compiling or hardware testing."""
    source = re.sub(r"/\*.*?\*/|//[^\n]*", "", source, flags=re.S)
    table = re.search(
        r"static\s+const\s+struct\s+dmi_system_id\s+tuxedo_dmi_string_match\[\]\s*=\s*\{(.*?)\n\};",
        source, re.S,
    )
    if not table:
        raise SyncError("Compatibility table changed; manual port required")
    entries = re.findall(r"\.matches\s*=\s*\{([^{}]*)\}", table[1], re.S)
    for brand in ("MECHREVO", "TUXEDO"):
        for field in FIELDS:
            expected = re.compile(rf'\s*DMI_MATCH\(\s*{field}\s*,\s*"{brand}"\s*\)\s*,\s*')
            if not any(expected.fullmatch(entry) for entry in entries):
                raise SyncError(f"Missing independent {brand} match for {field}")
    if not re.search(r"dmi_check_system\(\s*tuxedo_dmi_string_match\s*\)", source):
        raise SyncError("Compatibility table is no longer used")


def ensure_pr(root, repo, tag, release, base):
    title = f"chore: sync TUXEDO {tag}"
    body = (
        f"Automatically prepared from upstream **{tag}** (`{release}`).\n\n"
        f"Upstream: https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/tags/{tag}\n\n"
        f"Base main: `{base}`. The candidate merges the stable release into main, "
        "retaining the MECHREVO DMI entries. The upstream branch mirrors development main separately.\n\n"
        "- [x] Merge completed without conflicts\n"
        "- [x] MECHREVO and TUXEDO source guards passed\n"
        "- [ ] Compile against supported kernel headers locally\n"
        "- [ ] Review changes and validate affected hardware before merging\n\n"
        "No remote build, hardware test or automatic merge has been performed. "
        "Brand recognition does not certify EC/fan/power-control safety.\n"
    )
    prs = json.loads(run(
        "gh", "pr", "list", "--repo", repo, "--base", "main", "--head", PR_BRANCH,
        "--state", "open", "--json", "number", cwd=root,
    ).stdout)
    if prs:
        # Skip unchanged body/title to avoid daily notification churn.
        current = json.loads(run(
            "gh", "pr", "view", str(prs[0]["number"]), "--repo", repo,
            "--json", "title,body", cwd=root,
        ).stdout)
        if current != {"title": title, "body": body}:
            run("gh", "pr", "edit", str(prs[0]["number"]), "--repo", repo,
                "--title", title, "--body", body, cwd=root)
    else:
        run("gh", "pr", "create", "--repo", repo, "--base", "main", "--head", PR_BRANCH,
            "--title", title, "--body", body, cwd=root)


def sync(root, repo, upstream_url=UPSTREAM_URL, dry_run=False):
    root = Path(root).resolve()
    if git(root, "status", "--porcelain").stdout.strip():
        raise SyncError("Working tree must be clean (commit local changes first)")
    # Upstream tags live in their own namespace, so downstream tags cannot select a release.
    # Deliberately do not force tag updates: rewritten release tags stop the job.
    git(root, "fetch", "--no-tags", upstream_url,
        "+refs/heads/main:refs/remotes/tuxedo/main",
        "refs/tags/v*:refs/tags/upstream/v*")
    git(root, "fetch", "--prune", "--no-tags", "origin",
        "+refs/heads/*:refs/remotes/origin/*")
    tip = git(root, "rev-parse", "refs/remotes/tuxedo/main").stdout.strip()
    base = git(root, "rev-parse", "refs/remotes/origin/main").stdout.strip()
    mirror = "refs/remotes/origin/upstream"
    if git(root, "show-ref", "--verify", "--quiet", mirror, check=False).returncode == 0:
        if not ancestor(root, mirror, tip):
            raise SyncError("Upstream main was rewritten; refusing to overwrite upstream mirror")
    release_ref = latest_release(git(
        root, "for-each-ref", "--format=%(refname)", "refs/tags/upstream/",
    ).stdout.splitlines())
    tag = release_ref.rsplit("/", 1)[1]
    release = git(root, "rev-parse", f"{release_ref}^{{commit}}").stdout.strip()
    if not ancestor(root, release, tip):
        raise SyncError(f"Latest release {tag} is not on upstream main; manual review required")
    validate_mechrevo(git(root, "show", f"{base}:{COMPAT_PATH}").stdout)
    print(f"upstream main={tip}; stable={tag} ({release}); main={base}", flush=True)
    if not dry_run:
        # No force and atomic: preserve both mirror history and existing original release tags.
        git(root, "push", "--atomic", "origin", f"{tip}:refs/heads/upstream",
            f"{release_ref}:refs/tags/{tag}")
    if ancestor(root, release, base):
        print("Maintenance main already contains the latest stable release; no PR needed.")
        return
    parent = root / ".sync-worktrees"
    parent.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="candidate-", dir=parent) as directory:
        worktree = Path(directory) / "tree"
        git(root, "worktree", "add", "--detach", str(worktree), base)
        try:
            git(worktree, "-c", "user.name=mechrevo-sync[bot]",
                "-c", "user.email=mechrevo-sync@users.noreply.github.com",
                "-c", "commit.gpgsign=false", "merge", "--no-ff", "--no-edit", release,
                "-m", f"Merge TUXEDO {tag} into MECHREVO maintenance branch")
            validate_mechrevo((worktree / COMPAT_PATH).read_text())
            candidate = git(worktree, "rev-parse", "HEAD").stdout.strip()
            if dry_run:
                print(f"Dry run: validated candidate {candidate}; no remote refs or PRs changed.")
                return
            # Detect maintainer updates during merge preparation.
            live_base = git(root, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
            if live_base != base:
                raise SyncError("main changed during sync; rerun instead of publishing a stale candidate")
            previous = f"refs/remotes/origin/{PR_BRANCH}"
            exists = git(root, "show-ref", "--verify", "--quiet", previous, check=False).returncode == 0
            old = git(root, "rev-parse", previous).stdout.strip() if exists else ""
            # Idempotence: the same base/release can generate a different merge timestamp,
            # so compare tree and parents before replacing an existing bot candidate.
            if exists:
                old_shape = git(root, "show", "-s", "--format=%T %P", old).stdout
                new_shape = git(root, "show", "-s", "--format=%T %P", candidate).stdout
                if old_shape == new_shape:
                    candidate = old
            if candidate != old:
                # Only the documented bot-owned PR branch can be rewritten, using an exact lease.
                git(root, "push", f"--force-with-lease=refs/heads/{PR_BRANCH}:{old}", "origin",
                    f"{candidate}:refs/heads/{PR_BRANCH}")
            ensure_pr(root, repo, tag, release, base)
            print(f"Prepared {PR_BRANCH} for {tag}; main remains unchanged.")
        finally:
            git(worktree, "merge", "--abort", check=False)
            git(root, "worktree", "remove", "--force", str(worktree))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--upstream-url", default=UPSTREAM_URL)
    parser.add_argument("--dry-run", action="store_true", help="Fetch/merge locally; never push or call gh")
    args = parser.parse_args()
    if not args.repo and not args.dry_run:
        parser.error("--repo OWNER/REPO or GITHUB_REPOSITORY is required for PR operations")
    try:
        sync(Path.cwd(), args.repo, args.upstream_url, args.dry_run)
    except (SyncError, OSError, ValueError) as error:
        print(f"Upstream sync stopped: {error}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
