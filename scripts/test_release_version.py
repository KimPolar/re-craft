import subprocess
import tempfile
import unittest
from pathlib import Path

import release_version as rv


class ReleaseVersionTests(unittest.TestCase):
    def test_bump_rules_and_semver(self):
        self.assertEqual(rv.bump_kind(["fix(camera): correct look", "feat(world): load chunks"]), "minor")
        self.assertEqual(rv.bump_kind(["feat(world)!: change saves"]), "major")
        self.assertEqual(rv.bump_kind(["refactor(io): update\n\nBREAKING CHANGE: old format removed"]), "major")
        self.assertEqual(rv.bump_kind(["docs(readme): update"]), "patch")
        self.assertEqual(rv.bump_kind(["Legacy commit"]), "patch")
        self.assertEqual(rv.bumped((0, 4, 9), "minor"), (0, 5, 0))
        self.assertEqual(rv.bumped((0, 4, 9), "major"), (1, 0, 0))
        self.assertIsNone(rv.parse_tag("v01.0.0"))
        self.assertIsNone(rv.parse_tag("v1.2.3-beta.1"))

    def test_stamping_changes_only_workspace_versions(self):
        source = Path(__file__).resolve().parent.parent
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in ["Cargo.toml", "Cargo.lock", *(
                f"crates/{name}/Cargo.toml"
                for name in ["core", "native-client", "persistence", "resources", "session", "world"]
            )]:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes((source / relative).read_bytes())
            before = rv.tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
            rv.stamp(root, "2.3.4")
            after = rv.tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
            self.assertEqual(len(before["package"]), len(after["package"]))
            for old, new in zip(before["package"], after["package"]):
                if old["name"].startswith("native-") and "source" not in old:
                    self.assertEqual(new["version"], "2.3.4")
                    self.assertEqual({**old, "version": "2.3.4"}, new)
                else:
                    self.assertEqual(old, new)
            self.assertEqual(rv.tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"], "2.3.4")
            with self.assertRaises(ValueError):
                rv.stamp(root, "invalid")

    def test_push_range_and_retry_use_stable_tags(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n', encoding="utf-8")
            def commit(message):
                subprocess.run(["git", "add", "."], cwd=root, check=True)
                subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", message], cwd=root, check=True)
                return rv.git("rev-parse", "HEAD", root=root).strip()
            first = commit("chore(repo): start")
            subprocess.run(["git", "tag", "v0.1.0"], cwd=root, check=True)
            middle = commit("fix(camera): repair")
            head = commit("feat(world): add feature")
            self.assertEqual(rv.plan(root, first, head), ("0.2.0", "minor"))
            subprocess.run(["git", "tag", "v0.2.0"], cwd=root, check=True)
            self.assertEqual(rv.plan(root, first, head), ("0.2.0", "existing"))
            newer = commit("fix(input): correct")
            self.assertEqual(rv.plan(root, head, newer), ("0.2.1", "patch"))
            self.assertEqual(rv.plan(root, head, newer, {"v0.1.0"}), ("0.3.0", "minor"))
            self.assertEqual(rv.plan(root, "", first, {"v0.1.0", "v0.2.0"}), ("0.1.0", "existing"))
            self.assertEqual(rv.plan(root, "", first, {"v0.2.0"}), ("0.1.0", "superseded"))
            self.assertEqual(rv.plan(root, first, middle, {"v0.2.0"}), ("0.2.1", "superseded"))
            self.assertEqual(rv.plan(root, "0" * 40, newer), ("0.2.1", "patch"))
            rv.validate(root, first, newer)
            bad = commit("unformatted commit")
            with self.assertRaises(ValueError):
                rv.validate(root, newer, bad)


if __name__ == "__main__":
    unittest.main()
