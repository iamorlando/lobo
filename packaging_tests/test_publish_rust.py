import unittest
from unittest.mock import Mock, patch
import urllib.error

from scripts.publish_rust import already_published, publish_command, validate_release


class RustReleaseTests(unittest.TestCase):
    def setUp(self):
        self.workspace = {
            "package": {"version": "0.1.0"},
            "dependencies": {"lobo_books": {"path": "rust/crates/lobo_books", "version": "0.1.0"}},
        }
        self.packages = [
            {"name": "lobo-rs", "version": "0.1.0", "publish": None},
            {"name": "lobo_books", "version": "0.1.0", "publish": None},
            {"name": "lobo_py", "version": "0.1.0", "publish": []},
        ]

    def test_release_tags_match_workspace_version(self):
        for tag in ["rust-v0.1.0", "v0.1.0"]:
            self.assertEqual(validate_release(tag, self.workspace, self.packages), "0.1.0")
        for tag in ["master", "0.1.0", "rust-v0.2.0", "v0.1.0-extra"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_release(tag, self.workspace, self.packages)

    def test_mixed_package_versions_and_stale_requirements_are_rejected(self):
        self.packages[1]["version"] = "0.0.9"
        with self.assertRaises(ValueError):
            validate_release("v0.1.0", self.workspace, self.packages)
        self.packages[1]["version"] = "0.1.0"
        self.workspace["dependencies"]["lobo_books"]["version"] = "0.0.9"
        with self.assertRaises(ValueError):
            validate_release("v0.1.0", self.workspace, self.packages)

    def test_first_release_is_one_batch_and_never_queries_private_packages(self):
        exists = Mock(return_value=False)
        self.assertEqual(publish_command(self.packages, exists), [
            "cargo", "publish", "--workspace", "--registry", "crates-io", "--exclude", "lobo_py",
        ])
        self.assertEqual(exists.call_count, 2)

    def test_partial_release_skips_uploaded_versions(self):
        command = publish_command(self.packages, lambda package: package["name"] == "lobo_books")
        self.assertEqual(command[-4:], ["--exclude", "lobo_books", "--exclude", "lobo_py"])
        self.assertIsNone(publish_command(self.packages, lambda package: True))

    def test_only_registry_not_found_is_treated_as_unpublished(self):
        for status in [404, 403, 429, 500]:
            error = urllib.error.HTTPError("https://crates.io", status, "failure", {}, None)
            with self.subTest(status=status), patch("urllib.request.urlopen", side_effect=error):
                if status == 404:
                    self.assertFalse(already_published(self.packages[0]))
                else:
                    with self.assertRaises(urllib.error.HTTPError):
                        already_published(self.packages[0])


if __name__ == "__main__":
    unittest.main()
