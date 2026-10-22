"""Emit a deterministic CycloneDX 1.5 BOM from Cargo's target-filtered graph."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import quote


def lock_checksums(text):
    # Cargo writes these scalar fields as single-line JSON-compatible TOML strings.
    checksums = {}
    for block in text.split("[[package]]")[1:]:
        fields = {key: json.loads(value) for key, value in re.findall(
            r'^(name|version|source|checksum) = ("[^"\n]*")$', block, re.MULTILINE)}
        if all(key in fields for key in ("name", "version", "source", "checksum")):
            checksums[(fields["name"], fields["version"], fields["source"])] = fields["checksum"]
    return checksums


def make_bom(metadata, package, checksums):
    packages = {item["id"]: item for item in metadata["packages"]}
    roots = [item for item in packages.values()
             if item["name"] == package and item["id"] in metadata["workspace_members"]]
    if len(roots) != 1:
        raise ValueError(f"Expected one workspace package named {package}")
    root = roots[0]
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    edges = {}
    pending = [root["id"]]
    while pending:
        ref = pending.pop()
        if ref in edges:
            continue
        edges[ref] = {dep["pkg"] for dep in nodes[ref]["deps"]
                      if any(kind["kind"] != "dev" for kind in dep["dep_kinds"])}
        pending.extend(edges[ref] - edges.keys())

    def component(ref):
        item = packages[ref]
        purl = f"pkg:cargo/{quote(item['name'], safe='')}@{quote(item['version'], safe='')}"
        source = item.get("source")
        if source and not source.startswith("registry+"):
            purl += "?vcs_url=" + quote(source, safe="")
        result = {"type": "application" if ref == root["id"] else "library",
                  "bom-ref": purl, "name": item["name"], "version": item["version"], "purl": purl}
        if item.get("license"):
            result["licenses"] = [{"expression": item["license"]}]
        checksum = checksums.get((item["name"], item["version"], source))
        if checksum:
            result["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        return result

    components = {ref: component(ref) for ref in edges}
    return {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
            "metadata": {"component": components[root["id"]]},
            "components": sorted((value for ref, value in components.items() if ref != root["id"]),
                                 key=lambda value: value["bom-ref"]),
            "dependencies": sorted(({"ref": components[ref]["bom-ref"],
                                     "dependsOn": sorted(components[dep]["bom-ref"] for dep in deps)}
                                    for ref, deps in edges.items()), key=lambda value: value["ref"])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", default="wordcraft", choices=("wordcraft", "wordcraft-web"))
    parser.add_argument("--target", help="Release target triple (defaults to wasm32-unknown-unknown for web, otherwise the rustc host)")
    parser.add_argument("--output", type=Path, help="Write to this file instead of stdout")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    target = args.target or ("wasm32-unknown-unknown" if args.package == "wordcraft-web" else None)
    if not target:
        version = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target,
         "--manifest-path", str(root / "apps" / args.package / "Cargo.toml")], cwd=root, text=True))
    checksums = lock_checksums((Path(metadata["workspace_root"]) / "Cargo.lock").read_text(encoding="utf-8"))
    output = json.dumps(make_bom(metadata, args.package, checksums), indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(output, encoding="utf-8")
    else:
        sys.stdout.write(output)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, StopIteration, subprocess.CalledProcessError) as error:
        print(f"SBOM generation failed: {error}", file=sys.stderr)
        sys.exit(1)
