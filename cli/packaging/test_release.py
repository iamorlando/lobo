import hashlib
import importlib.util
import io
import tarfile
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_formula_uses_each_real_archive_digest_and_no_dependencies(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            for target in release.TARGETS:
                with tarfile.open(root / f"lobo-0.1.0-{target}.tar.gz", "w:gz") as tar:
                    entry = tarfile.TarInfo("bin/lobo")
                    entry.size = len(target)
                    entry.mode = 0o755
                    tar.addfile(entry, io.BytesIO(target.encode()))
            output = root / "Formula/lobo.rb"
            release.formula(root, "0.1.0", "https://example.com/releases/cli-v0.1.0", output)
            text = output.read_text()
            for target in release.TARGETS:
                archive = root / f"lobo-0.1.0-{target}.tar.gz"
                self.assertIn(hashlib.sha256(archive.read_bytes()).hexdigest(), text)
                packaged = root / f"lobo-0.1.0.{release.BOTTLE_TAGS[target]}.bottle.tar.gz"
                self.assertIn(hashlib.sha256(packaged.read_bytes()).hexdigest(), text)
                with tarfile.open(packaged) as bottle:
                    executable = bottle.getmember("lobo/0.1.0/bin/lobo")
                    self.assertEqual(bottle.extractfile(executable).read(), target.encode())
                    self.assertEqual(executable.mode, 0o755)
                    self.assertIn("lobo/0.1.0/INSTALL_RECEIPT.json", bottle.getnames())
            self.assertNotIn("depends_on", text)
            self.assertNotIn("@PLATFORMS@", text)
            self.assertNotIn("@BOTTLES@", text)
            self.assertIn("any_skip_relocation", text)
            self.assertEqual(len((root / "SHA256SUMS").read_text().splitlines()), 8)

    def test_incomplete_release_cannot_generate_installable_formula(self):
        with tempfile.TemporaryDirectory() as d:
            output = Path(d) / "lobo.rb"
            with self.assertRaises(ValueError):
                release.formula(Path(d), "0.1.0", "https://example.com", output)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
