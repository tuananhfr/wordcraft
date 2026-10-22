"""Small synthetic Cargo graphs; no Cargo invocation or network needed."""
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from cargo_sbom import lock_checksums, main, make_bom


class SbomTests(unittest.TestCase):
    def setUp(self):
        self.source = "registry+https://github.com/rust-lang/crates.io-index"
        self.checksum = "ab" * 32
        self.metadata = {
            "workspace_members": ["app", "web", "local"],
            "packages": [
                {"id": "app", "name": "wordcraft", "version": "2.0.0", "source": None, "license": "MIT OR Apache-2.0"},
                {"id": "web", "name": "wordcraft-web", "version": "2.0.0", "source": None},
                {"id": "local", "name": "local", "version": "2.0.0", "source": None},
                {"id": "dep", "name": "dep", "version": "1.0.0", "source": self.source, "license": "MIT"},
                {"id": "dev", "name": "test-only", "version": "1.0.0", "source": self.source}],
            "resolve": {"nodes": [
                {"id": "app", "deps": [self.dep("local"), self.dep("dep", "build"), self.dep("dev", "dev")]},
                {"id": "web", "deps": [self.dep("local")]},
                {"id": "local", "deps": [self.dep("dep")]},
                {"id": "dep", "deps": []}, {"id": "dev", "deps": []}]}}
        self.checksums = lock_checksums(
            f'[[package]]\nname = "dep"\nversion = "1.0.0"\nsource = "{self.source}"\nchecksum = "{self.checksum}"\n')

    @staticmethod
    def dep(package, kind=None):
        return {"pkg": package, "dep_kinds": [{"kind": kind, "target": None}]}

    def test_desktop_graph_licenses_hashes_and_edges(self):
        bom = make_bom(self.metadata, "wordcraft", self.checksums)
        self.assertEqual((bom["bomFormat"], bom["specVersion"]), ("CycloneDX", "1.5"))
        app = bom["metadata"]["component"]
        self.assertEqual(app["purl"], "pkg:cargo/wordcraft@2.0.0")
        self.assertEqual(app["type"], "application")
        self.assertEqual(app["licenses"], [{"expression": "MIT OR Apache-2.0"}])
        components = {item["name"]: item for item in bom["components"]}
        self.assertEqual(set(components), {"local", "dep"})
        self.assertNotIn("hashes", components["local"])
        self.assertEqual(components["local"]["type"], "library")
        self.assertEqual(components["dep"]["hashes"], [{"alg": "SHA-256", "content": self.checksum}])
        edges = {item["ref"]: item["dependsOn"] for item in bom["dependencies"]}
        self.assertEqual(edges[app["bom-ref"]], ["pkg:cargo/dep@1.0.0", "pkg:cargo/local@2.0.0"])
        self.assertEqual(edges[components["local"]["bom-ref"]], ["pkg:cargo/dep@1.0.0"])
        self.assertEqual(edges[components["dep"]["bom-ref"]], [])

    def test_web_package_selection(self):
        bom = make_bom(self.metadata, "wordcraft-web", self.checksums)
        self.assertEqual(bom["metadata"]["component"]["name"], "wordcraft-web")
        self.assertEqual({item["name"] for item in bom["components"]}, {"local", "dep"})

    def test_order_independent_output_and_cycle(self):
        self.metadata["resolve"]["nodes"][3]["deps"] = [self.dep("local")]
        original = make_bom(self.metadata, "wordcraft", self.checksums)
        shuffled = copy.deepcopy(self.metadata)
        shuffled["packages"].reverse()
        shuffled["resolve"]["nodes"].reverse()
        for node in shuffled["resolve"]["nodes"]:
            node["deps"].reverse()
        self.assertEqual(original, make_bom(shuffled, "wordcraft", self.checksums))

    def test_cli_uses_locked_target_filtered_metadata(self):
        for package, options, target in [
            ("wordcraft", ["--target", "x86_64-unknown-linux-gnu"], "x86_64-unknown-linux-gnu"),
            ("wordcraft-web", [], "wasm32-unknown-unknown"),
        ]:
            with self.subTest(package=package), tempfile.TemporaryDirectory() as temporary:
                metadata = dict(self.metadata, workspace_root=temporary)
                (Path(temporary) / "Cargo.lock").write_text("", encoding="utf-8")
                output = io.StringIO()
                with patch("sys.argv", ["cargo_sbom.py", "--package", package] + options), \
                        patch("cargo_sbom.subprocess.check_output", return_value=json.dumps(metadata)) as run, \
                        patch("sys.stdout", output):
                    main()
                command = run.call_args.args[0]
                self.assertEqual(command[:7], ["cargo", "metadata", "--format-version", "1", "--locked",
                                               "--filter-platform", target])
                self.assertTrue(command[-1].endswith(f"apps/{package}/Cargo.toml"))
                self.assertEqual(json.loads(output.getvalue())["metadata"]["component"]["name"], package)

    def test_missing_package_rejected(self):
        with self.assertRaises(ValueError):
            make_bom(self.metadata, "missing", self.checksums)

    def test_checksum_source_and_version_must_match(self):
        self.assertNotIn(("dep", "2.0.0", self.source), self.checksums)
        self.metadata["packages"][3]["source"] = "git+https://example.org/dep#abc"
        bom = make_bom(self.metadata, "wordcraft", self.checksums)
        dep = next(item for item in bom["components"] if item["name"] == "dep")
        self.assertNotIn("hashes", dep)
        self.assertIn("?vcs_url=", dep["purl"])


if __name__ == "__main__":
    unittest.main()
