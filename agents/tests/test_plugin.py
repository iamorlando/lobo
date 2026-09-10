"""Portable regression tests for the independently installed skills."""

import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
import zipfile
from email.message import Message
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from urllib.error import HTTPError

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "plugins" / "lobo-adapters"
SKILLS = ROOT.parent / "skills"


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


helper = module("helper", ROOT / "scripts" / "lobo_agent.py")
packager = module("packager", ROOT / "scripts" / "package.py")
browser = module(
    "browser", SKILLS / "lobo-adapter-tests/scripts/browser_smoke.py"
)


def metadata(repository="iamorlando/lobo"):
    result = Message()
    result["Name"] = "lobo"
    result["Version"] = "0.1.0"
    result["Project-URL"] = f"Repository, https://github.com/{repository}"
    return result


class SetupTests(unittest.TestCase):
    def test_direct_source_inputs_never_reach_pip(self):
        for source in (
            "https://example.com/lobo.tar.gz",
            "./source",
            "git+https://github.com/iamorlando/lobo",
            "lobo @ https://example.com/source.zip",
        ):
            with self.subTest(source=source), self.assertRaises(RuntimeError):
                helper.wheel_request(source)
        self.assertEqual(helper.wheel_request("lobo==0.1.0"), "lobo==0.1.0")
        self.assertEqual(
            helper.wheel_request("https://example.com/lobo.whl?download=1"),
            "https://example.com/lobo.whl?download=1",
        )

    def test_optional_authentication_is_bounded_and_process_stops(self):
        note = helper.register_codex(
            [sys.executable, "-c", "import time;time.sleep(30)"], timeout=0.1
        )
        self.assertIn("authentication timed out", note)

    def test_wrong_pypi_package_rejected_before_install(self):
        with self.assertRaisesRegex(RuntimeError, "different package"):
            helper.validate_metadata(metadata("boroivanov/lobo"))
        self.assertEqual(helper.validate_metadata(metadata()), "iamorlando/lobo")
        self.assertEqual(
            helper.validate_metadata(metadata("iamorlando/loblib")), "iamorlando/loblib"
        )

    def test_mcp_defaults_to_existing_tools_without_a_vendor_cli(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/lobo_agent.py"), "mcp"],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        self.assertEqual(
            json.loads(result.stdout), {"mcp": "Using the agent's existing tools"}
        )

    def test_read_metadata_from_another_interpreter_without_lobo_import(self):
        output = json.dumps(list(metadata().items()))
        with patch.object(
            helper,
            "run",
            return_value=subprocess.CompletedProcess([], 0, stdout=output),
        ) as run:
            found = helper.installed_metadata("consumer-python", "lobo")
        self.assertEqual(found["Version"], "0.1.0")
        self.assertEqual(run.call_args.args[0][0], "consumer-python")
        self.assertNotIn("import lobo", run.call_args.args[0][2])

    def test_windows_and_unix_environment_paths(self):
        with patch.object(helper, "os", SimpleNamespace(name="nt")):
            self.assertEqual(
                helper.python_in("consumer").as_posix().split("/")[-2:],
                ["Scripts", "python.exe"],
            )
        with patch.object(helper, "os", SimpleNamespace(name="posix")):
            self.assertEqual(
                helper.python_in("consumer").as_posix().split("/")[-2:],
                ["bin", "python"],
            )

    def test_mcp_merge_preserves_existing_config_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as temporary, contextlib.chdir(temporary):
            path = Path(".mcp.json")
            original = {
                "mcpServers": {
                    "Context7": {"url": "https://existing.example/mcp"},
                    "personal": {"command": "my-server"},
                },
                "custom": True,
            }
            path.write_text(json.dumps(original))
            helper.configure_mcp("claude", testing=True)
            once = path.read_text()
            helper.configure_mcp("claude", testing=True)
            self.assertEqual(path.read_text(), once)
            merged = json.loads(once)
            self.assertEqual(
                merged["mcpServers"]["Context7"], original["mcpServers"]["Context7"]
            )
            self.assertIn("playwright", merged["mcpServers"])
            self.assertNotIn("context7", merged["mcpServers"])
            self.assertTrue(merged["custom"])

    def test_mcp_failure_is_nonblocking_and_does_not_echo_config(self):
        with patch.object(helper.shutil, "which", return_value=None):
            result = helper.configure_mcp("codex")
        self.assertIn("official docs", result["mcp"])

    def test_remote_examples_record_default_branch_fallback(self):
        def github(path):
            if "/commits/v0.1.0" in path or "/commits/0.1.0" in path:
                raise HTTPError(path, 404, "missing", {}, None)
            if path == "repos/iamorlando/lobo":
                return {"default_branch": "release-branch"}
            if "/commits/release-branch" in path:
                return {"sha": "abc123"}
            if "/contents/python/examples/kraken?ref=abc123" in path:
                return [
                    {"name": "adapter.py", "path": "python/examples/kraken/adapter.py"}
                ]
            if "/contents/python/examples/kraken/adapter.py?ref=abc123" in path:
                return {"content": "IyByZW1vdGUgZXhhbXBsZQo="}
            self.fail(f"Unexpected remote request: {path}")

        with tempfile.TemporaryDirectory() as temporary:
            args = argparse.Namespace(
                python="consumer-python",
                distribution="lobo",
                ref=None,
                out=temporary,
                example="kraken",
                tests=False,
            )
            with (
                patch.object(helper, "installed_metadata", return_value=metadata()),
                patch.object(helper, "github", side_effect=github),
            ):
                result = helper.references(args)
            self.assertFalse(result["version_matched_ref"])
            self.assertEqual(result["reference_source"], "default_branch")
            self.assertEqual(result["revision"], "abc123")
            self.assertEqual(
                (Path(temporary) / "adapter.py").read_text(), "# remote example\n"
            )
            self.assertIn("/blob/abc123/", result["files"][0]["url"])


class BundleTests(unittest.TestCase):
    def test_each_skill_runs_alone_after_unpacking_elsewhere(self):
        packager.synchronize(check=True)
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            for name in packager.SKILL_NAMES:
                skill = SKILLS / name
                bundle = destination / (skill.name + ".zip")
                digest = packager.archive(skill, bundle)
                self.assertEqual(digest, packager.archive(skill, bundle))
                with zipfile.ZipFile(bundle) as archive:
                    archive.extractall(destination / "consumer")
                copied = destination / "consumer" / skill.name
                # Every linked guide/example/test must survive installing just
                # this skill, with neither its sibling nor repository sources.
                for guide in copied.rglob("*.md"):
                    for link in re.findall(r"\[[^\]]*\]\(([^)]+)\)", guide.read_text()):
                        if "://" in link or link.startswith("#"):
                            continue
                        target = (guide.parent / link.split("#")[0]).resolve()
                        self.assertTrue(target.is_relative_to(copied.resolve()), link)
                        self.assertTrue(target.exists(), f"{guide}: {link}")
                provenance = json.loads((copied / "references/sources.json").read_text())
                self.assertTrue(
                    {
                        "web/README.md",
                        "rust/crates/lobo_server/README.md",
                        "rust/crates/lobo_replay/src/custom/README.md",
                    }.issubset({entry["source"] for entry in provenance["files"]})
                )
                for entry in provenance["files"]:
                    self.assertEqual(
                        hashlib.sha256((copied / entry["file"]).read_bytes()).hexdigest(),
                        entry["bundled_sha256"],
                    )
                self.assertFalse((copied / "agents/openai.yaml").exists())
                for script in (copied / "scripts").glob("*.py"):
                    result = subprocess.run(
                        [sys.executable, str(script), "--help"],
                        cwd=destination,
                        env={**os.environ, "PYTHONPATH": ""},
                        capture_output=True,
                        text=True,
                        timeout=10,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                for path in copied.rglob("*"):
                    if path.is_file():
                        self.assertNotIn(b"/Users/orlando", path.read_bytes())

    def test_native_plugin_dependencies_are_included(self):
        manifest = json.loads((PLUGIN / ".codex-plugin/plugin.json").read_text())
        servers = json.loads((PLUGIN / manifest["mcpServers"]).read_text())[
            "mcpServers"
        ]
        self.assertEqual(servers["context7"]["url"], "https://mcp.context7.com/mcp")
        self.assertIn("@playwright/mcp@latest", servers["playwright"]["args"])

    def test_server_failure_is_cleaned_up(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            server_file = destination / "server.py"
            server_file.write_text(
                "import time\nprint('started without a URL', flush=True)\ntime.sleep(30)\n"
            )
            server = browser.LocalServer(server_file, [], 0.3, destination)
            try:
                with self.assertRaisesRegex(RuntimeError, "timeout"):
                    server.start()
            finally:
                server.stop()
            self.assertIsNotNone(server.process.poll())
            self.assertIn(
                "started without a URL", (destination / "server.log").read_text()
            )


if __name__ == "__main__":
    unittest.main()
