#!/usr/bin/env python3
"""Generate a deterministic CycloneDX SBOM from committed lockfiles."""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
from pathlib import Path, PurePosixPath
import tomllib
from urllib.parse import quote
import uuid

from release_artifacts import REPOSITORY_ROOT, sha256, validate_repository_version


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def component_ref(ecosystem: str, name: str, version: str, source: str = "") -> str:
    identity = f"{ecosystem}\0{name}\0{version}\0{source}".encode()
    digest = hashlib.sha256(identity).hexdigest()[:24]
    return f"urn:api-subway:component:{ecosystem}:{digest}"


def cargo_purl(name: str, version: str) -> str:
    return f"pkg:cargo/{quote(name, safe='')}@{quote(version, safe='')}"


def npm_purl(name: str, version: str) -> str:
    return f"pkg:npm/{quote(name, safe='/')}@{quote(version, safe='')}"


def cargo_components(lockfile: Path) -> tuple[list[dict[str, object]], dict[str, list[str]]]:
    lock = tomllib.loads(lockfile.read_text(encoding="utf-8"))
    packages = lock.get("package", [])
    components: list[dict[str, object]] = []
    refs_by_identity: dict[tuple[str, str], list[str]] = {}
    raw_dependencies: dict[str, list[str]] = {}

    for package in packages:
        name = str(package["name"])
        version = str(package["version"])
        source = str(package.get("source", "workspace"))
        bom_ref = component_ref("cargo", name, version, source)
        component: dict[str, object] = {
            "type": "library",
            "bom-ref": bom_ref,
            "name": name,
            "version": version,
            "purl": cargo_purl(name, version),
            "properties": [
                {"name": "api-subway:ecosystem", "value": "cargo"},
                {"name": "api-subway:source", "value": source},
            ],
        }
        checksum = package.get("checksum")
        if isinstance(checksum, str):
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        components.append(component)
        refs_by_identity.setdefault((name, version), []).append(bom_ref)
        raw_dependencies[bom_ref] = [str(value) for value in package.get("dependencies", [])]

    dependencies: dict[str, list[str]] = {}
    refs_by_name: dict[str, list[str]] = {}
    for (name, _), refs in refs_by_identity.items():
        refs_by_name.setdefault(name, []).extend(refs)
    for bom_ref, raw_values in raw_dependencies.items():
        resolved: set[str] = set()
        for value in raw_values:
            parts = value.split()
            name = parts[0]
            version = parts[1] if len(parts) > 1 and parts[1][0].isdigit() else None
            candidates = (
                refs_by_identity.get((name, version), [])
                if version is not None
                else refs_by_name.get(name, [])
            )
            resolved.update(candidates)
        dependencies[bom_ref] = sorted(resolved)
    return components, dependencies


def npm_name_from_lock_path(path: str) -> str:
    marker = "node_modules/"
    if marker not in path:
        raise ValueError(f"unsupported npm lockfile path: {path!r}")
    return path.rsplit(marker, maxsplit=1)[1]


def npm_components(
    lockfile: Path,
) -> tuple[list[dict[str, object]], dict[str, list[str]], list[str]]:
    lock = json.loads(lockfile.read_text(encoding="utf-8"))
    packages = lock.get("packages")
    if not isinstance(packages, dict):
        raise ValueError("npm lockfile does not contain a packages object")

    components: list[dict[str, object]] = []
    refs_by_path: dict[str, str] = {}
    for path, package in sorted(packages.items()):
        if not path or not isinstance(package, dict):
            continue
        version = package.get("version")
        if not isinstance(version, str):
            raise ValueError(f"npm package {path!r} has no version")
        name = npm_name_from_lock_path(path)
        source = f"{package.get('resolved', '')}\0{path}"
        bom_ref = component_ref("npm", name, version, source)
        component: dict[str, object] = {
            "type": "library",
            "bom-ref": bom_ref,
            "name": name,
            "version": version,
            "purl": npm_purl(name, version),
            "scope": "optional" if package.get("optional") is True else "required",
            "properties": [
                {"name": "api-subway:ecosystem", "value": "npm"},
                {"name": "api-subway:scope", "value": "build"},
                {"name": "api-subway:lockfile-path", "value": path},
            ],
        }
        license_value = package.get("license")
        if isinstance(license_value, str) and license_value:
            component["licenses"] = [{"expression": license_value}]
        integrity = package.get("integrity")
        if isinstance(integrity, str):
            algorithm, separator, encoded = integrity.partition("-")
            algorithms = {"sha256": "SHA-256", "sha384": "SHA-384", "sha512": "SHA-512"}
            if separator and algorithm in algorithms:
                try:
                    content = base64.b64decode(encoded, validate=True).hex()
                except binascii.Error as error:
                    raise ValueError(f"invalid npm integrity for {name}@{version}") from error
                component["hashes"] = [{"alg": algorithms[algorithm], "content": content}]
        components.append(component)
        refs_by_path[path] = bom_ref

    dependencies: dict[str, list[str]] = {}
    for path, package in sorted(packages.items()):
        if not path or not isinstance(package, dict):
            continue
        bom_ref = refs_by_path[path]
        required = dependency_names(package.get("dependencies"), path)
        optional = dependency_names(package.get("optionalDependencies"), path)
        resolved: set[str] = set()
        for dependency in sorted(required | optional):
            dependency_path = resolve_npm_dependency_path(path, dependency, packages)
            if dependency_path is None:
                if dependency in optional:
                    continue
                raise ValueError(
                    f"npm dependency {dependency!r} for {path!r} is absent from the lockfile"
                )
            reference = refs_by_path.get(dependency_path)
            if reference is not None:
                resolved.add(reference)
        dependencies[bom_ref] = sorted(resolved)

    root = packages.get("")
    if not isinstance(root, dict):
        raise ValueError("npm lockfile does not contain the root package")
    root_names = set().union(
        dependency_names(root.get("dependencies"), "<root>"),
        dependency_names(root.get("devDependencies"), "<root>"),
        dependency_names(root.get("optionalDependencies"), "<root>"),
    )
    root_dependencies: list[str] = []
    for dependency in sorted(root_names):
        dependency_path = resolve_npm_dependency_path("", dependency, packages)
        if dependency_path is None:
            raise ValueError(
                f"root npm dependency {dependency!r} is absent from the lockfile"
            )
        root_dependencies.append(refs_by_path[dependency_path])
    return components, dependencies, sorted(root_dependencies)


def dependency_names(value: object, owner: str) -> set[str]:
    if value is None:
        return set()
    if not isinstance(value, dict) or any(not isinstance(name, str) for name in value):
        raise ValueError(f"npm dependencies for {owner!r} must be an object")
    return set(value)


def resolve_npm_dependency_path(
    owner_path: str,
    dependency: str,
    packages: dict[str, object],
) -> str | None:
    current = PurePosixPath(owner_path) if owner_path else PurePosixPath(".")
    while True:
        prefix = "" if current == PurePosixPath(".") else f"{current}/"
        candidate = f"{prefix}node_modules/{dependency}"
        if candidate in packages:
            return candidate
        if current == PurePosixPath("."):
            return None
        current = current.parent


def build_sbom(version: str) -> dict[str, object]:
    cargo_lock = REPOSITORY_ROOT / "Cargo.lock"
    npm_lock = REPOSITORY_ROOT / "crates/api-subway-renderer/viewer/package-lock.json"
    cargo, cargo_dependencies = cargo_components(cargo_lock)
    npm, npm_dependencies, npm_root_dependencies = npm_components(npm_lock)
    components = sorted(
        [*cargo, *npm],
        key=lambda item: (
            str(item["properties"][0]["value"]),
            str(item["name"]),
            str(item["version"]),
            str(item["bom-ref"]),
        ),
    )
    root_ref = f"pkg:generic/api-subway@{quote(version, safe='')}"
    all_refs = sorted(str(component["bom-ref"]) for component in components)
    cargo_root_dependencies = sorted(
        str(component["bom-ref"])
        for component in cargo
        if any(
            property_value.get("name") == "api-subway:source"
            and property_value.get("value") == "workspace"
            for property_value in component.get("properties", [])
            if isinstance(property_value, dict)
        )
    )
    dependency_entries = [
        {
            "ref": root_ref,
            "dependsOn": sorted(
                set(cargo_root_dependencies) | set(npm_root_dependencies)
            ),
        },
        *(
            {
                "ref": bom_ref,
                "dependsOn": dependencies,
            }
            for bom_ref, dependencies in sorted(cargo_dependencies.items())
        ),
        *(
            {
                "ref": bom_ref,
                "dependsOn": dependencies,
            }
            for bom_ref, dependencies in sorted(npm_dependencies.items())
        ),
    ]
    existing_refs = {entry["ref"] for entry in dependency_entries}
    dependency_entries.extend(
        {"ref": reference, "dependsOn": []}
        for reference in all_refs
        if reference not in existing_refs
    )
    dependency_entries.sort(key=lambda item: str(item["ref"]))

    serial_seed = f"api-subway:{version}:{sha256(cargo_lock)}:{sha256(npm_lock)}"
    serial = uuid.uuid5(uuid.NAMESPACE_URL, serial_seed)
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{serial}",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": root_ref,
                "name": "api-subway",
                "version": version,
                "purl": root_ref,
                "licenses": [{"expression": "MIT"}],
                "externalReferences": [
                    {
                        "type": "vcs",
                        "url": "https://github.com/api-subway/api-subway",
                    }
                ],
            },
            "properties": [
                {"name": "api-subway:lockfile:cargo:sha256", "value": sha256(cargo_lock)},
                {"name": "api-subway:lockfile:npm:sha256", "value": sha256(npm_lock)},
            ],
        },
        "components": components,
        "dependencies": dependency_entries,
    }


def main() -> int:
    args = parse_args()
    validate_repository_version(args.version)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    contents = json.dumps(
        build_sbom(args.version),
        ensure_ascii=False,
        indent=2,
        sort_keys=True,
    ) + "\n"
    temporary = output.with_name(f".{output.name}.tmp")
    temporary.write_text(contents, encoding="utf-8", newline="\n")
    temporary.replace(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
