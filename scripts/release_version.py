#!/usr/bin/env python3
"""Calculate a Re-Craft release and stamp the Cargo workspace for CI."""

from __future__ import annotations

import argparse
import re
import subprocess
import tomllib
from pathlib import Path

SEMVER = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
HEADER = re.compile(r"^(\w+)(?:\([^)\r\n]+\))?(!)?:\s+\S", re.IGNORECASE)
BREAKING = re.compile(r"^BREAKING[ -]CHANGE:\s*\S", re.MULTILINE | re.IGNORECASE)
CONVENTION = re.compile(
    r"^(feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert)"
    r"\([a-z0-9][a-z0-9._/-]*\)!?: \S.*$"
)


def parse_tag(tag: str) -> tuple[int, int, int] | None:
    match = SEMVER.fullmatch(tag.strip())
    return tuple(map(int, match.groups())) if match else None


def bump_kind(messages: list[str]) -> str:
    result = "patch"  # Every branch push produces a unique release.
    for message in messages:
        header = HEADER.match(message.strip().split("\n", 1)[0])
        if BREAKING.search(message) or (header and header.group(2)):
            return "major"
        if header and header.group(1).lower() == "feat":
            result = "minor"
    return result


def bumped(base: tuple[int, int, int], kind: str) -> tuple[int, int, int]:
    major, minor, patch = base
    if kind == "major":
        return (major + 1, 0, 0)
    if kind == "minor":
        return (major, minor + 1, 0)
    return (major, minor, patch + 1)


def git(*args: str, root: Path) -> str:
    return subprocess.check_output(["git", *args], cwd=root, text=True, encoding="utf-8")


def push_revisions(root: Path, before: str, head: str) -> list[str]:
    if before and set(before) != {"0"}:
        exists = subprocess.run(
            ["git", "cat-file", "-e", before + "^{commit}"], cwd=root,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        ).returncode == 0
        if exists:
            return [f"{before}..{head}"]
    return ["-1", head]


def validate(root: Path, before: str, head: str) -> None:
    revisions = push_revisions(root, before, head)
    lines = git("log", "--no-merges", "--format=%h %s", *revisions, root=root).splitlines()
    bad = [line for line in lines if not CONVENTION.fullmatch(line.split(" ", 1)[1])]
    if bad:
        raise ValueError("commit subjects must use type(scope): subject:\n" + "\n".join(bad))


def plan(root: Path, before: str, head: str, published_tags: set[str] | None = None) -> tuple[str, str]:
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace_version = manifest["workspace"]["package"]["version"]
    baseline = parse_tag("v" + workspace_version)
    if baseline is None:
        raise ValueError("workspace version must be a plain x.y.z version")
    tags = [v for line in git("tag", "--list", root=root).splitlines() if (v := parse_tag(line))]
    same_commit = [
        v for line in git("tag", "--points-at", head, root=root).splitlines()
        if (v := parse_tag(line))
    ]
    superseded = published_tags is not None and any(
        line in published_tags and parse_tag(line) is not None
        for line in git("tag", "--contains", head, root=root).splitlines()
    )
    if same_commit:
        # A previously published commit is an idempotent rerun. A failed old
        # draft must not become the latest release after a descendant ships.
        version = ".".join(map(str, max(same_commit)))
        already_published = published_tags is None or "v" + version in published_tags
        return version, "existing" if already_published or not superseded else "superseded"
    base = max([baseline, *tags])
    ancestors = [
        v for line in git("tag", "--merged", head, root=root).splitlines()
        if (published_tags is None or line in published_tags) and (v := parse_tag(line))
    ]
    # Include commits from failed earlier runs until a release has succeeded.
    revisions = ["v" + ".".join(map(str, max(ancestors))) + ".." + head] if ancestors else [head]
    messages = git("log", "--format=%B%x00", *revisions, root=root).split("\x00")
    kind = bump_kind(messages)
    return ".".join(map(str, bumped(base, kind))), "superseded" if superseded else kind


def stamp(root: Path, version: str) -> None:
    if parse_tag("v" + version) is None:
        raise ValueError("version must be a plain x.y.z version")
    manifest_path = root / "Cargo.toml"
    lock_path = root / "Cargo.lock"
    manifest_text = manifest_path.read_text(encoding="utf-8")
    manifest = tomllib.loads(manifest_text)
    old_version = manifest["workspace"]["package"]["version"]
    members = manifest["workspace"]["members"]
    names = set()
    for member in members:
        package = tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        if package["version"] != {"workspace": True}:
            raise ValueError(f"{member} does not inherit its workspace version")
        names.add(package["name"])
    pattern = r'(?ms)(^\[workspace\.package\]\s*\n.*?^version\s*=\s*")([^"\n]+)(")'
    new_manifest, count = re.subn(pattern, lambda m: m[1] + version + m[3], manifest_text, count=1)
    if count != 1:
        raise ValueError("cannot locate workspace version")
    lock_text = lock_path.read_text(encoding="utf-8")
    sections = re.split(r"(?=^\[\[package\]\]\s*$)", lock_text, flags=re.MULTILINE)
    found = set()
    for i, section in enumerate(sections):
        if not section.startswith("[[package]]"):
            continue
        package = tomllib.loads(section)["package"][0]
        name = package["name"]
        if name not in names or "source" in package:
            continue
        if name in found or package["version"] != old_version:
            raise ValueError(f"unexpected lockfile entry for {name}")
        sections[i], count = re.subn(
            r'(?m)^version = "[^"]+"$', f'version = "{version}"', section, count=1
        )
        if count != 1:
            raise ValueError(f"cannot stamp lockfile entry for {name}")
        found.add(name)
    if found != names:
        raise ValueError(f"missing workspace lockfile entries: {sorted(names - found)}")
    # Validate both files fully before changing either; CI uses a disposable checkout.
    tomllib.loads(new_manifest)
    tomllib.loads("".join(sections))
    manifest_path.write_text(new_manifest, encoding="utf-8", newline="\n")
    lock_path.write_text("".join(sections), encoding="utf-8", newline="\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("plan")
    p.add_argument("--before", default="")
    p.add_argument("--head", default="HEAD")
    p.add_argument("--published-tags-file")
    v = sub.add_parser("validate")
    v.add_argument("--before", default="")
    v.add_argument("--head", default="HEAD")
    s = sub.add_parser("stamp")
    s.add_argument("version")
    args = parser.parse_args()
    root = Path.cwd()
    if args.command == "plan":
        published = None if args.published_tags_file is None else set(
            Path(args.published_tags_file).read_text(encoding="utf-8").splitlines()
        )
        version, kind = plan(root, args.before, args.head, published)
        print(f"{version} {kind}")
    elif args.command == "validate":
        validate(root, args.before, args.head)
    else:
        stamp(root, args.version)


if __name__ == "__main__":
    main()
