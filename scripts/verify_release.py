#!/usr/bin/env python3
"""Validate release completeness, archive safety, SBOM shape, and checksums."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import stat
import tarfile
import zipfile

from release_artifacts import (
    RELEASE_TARGETS,
    validate_repository_version,
    verify_release_bundle,
)


MAX_ARCHIVE_ENTRY_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_CONTENT_BYTES = 256 * 1024 * 1024
MAX_ARCHIVE_MEMBERS = 256


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    return parser.parse_args()


def validate_member_names(archive: str, names: list[str]) -> None:
    if not names:
        raise ValueError(f"archive is empty: {archive}")
    if len(names) != len(set(names)):
        raise ValueError(f"archive contains duplicate member names: {archive}")
    if len(names) > MAX_ARCHIVE_MEMBERS:
        raise ValueError(
            f"archive contains more than {MAX_ARCHIVE_MEMBERS} members: {archive}"
        )
    for name in names:
        path = PurePosixPath(name)
        if (
            not name
            or path == PurePosixPath(".")
            or path.is_absolute()
            or ".." in path.parts
            or "\\" in name
        ):
            raise ValueError(f"unsafe archive path in {archive}: {name!r}")


def tar_entries(path: Path) -> dict[str, bytes]:
    with tarfile.open(path, mode="r|gz") as archive:
        entries: dict[str, bytes] = {}
        total_size = 0
        for member_number, member in enumerate(archive, start=1):
            if member_number > MAX_ARCHIVE_MEMBERS:
                raise ValueError(
                    f"archive contains more than {MAX_ARCHIVE_MEMBERS} members: {path.name}"
                )
            validate_member_names(path.name, [member.name])
            if member.name in entries:
                raise ValueError(
                    f"archive contains duplicate member names: {path.name}"
                )
            if not member.isfile():
                raise ValueError(
                    f"unsupported non-file member in {path.name}: {member.name!r}"
                )
            if member.size > MAX_ARCHIVE_ENTRY_BYTES:
                raise ValueError(f"oversized archive member in {path.name}: {member.name!r}")
            total_size += member.size
            if total_size > MAX_ARCHIVE_CONTENT_BYTES:
                raise ValueError(f"archive expands beyond 256 MiB: {path.name}")
            source = archive.extractfile(member)
            if source is None:
                raise ValueError(f"could not read {member.name!r} from {path.name}")
            contents = source.read(member.size + 1)
            if len(contents) != member.size:
                raise ValueError(f"truncated member {member.name!r} in {path.name}")
            entries[member.name] = contents
        if not entries:
            raise ValueError(f"archive is empty: {path.name}")
        return entries


def zip_entries(path: Path) -> dict[str, bytes]:
    with zipfile.ZipFile(path) as archive:
        members = archive.infolist()
        names = [member.filename for member in members]
        validate_member_names(path.name, names)
        total_size = 0
        entries: dict[str, bytes] = {}
        for member in members:
            if member.is_dir():
                raise ValueError(
                    f"unsupported directory member in {path.name}: {member.filename!r}"
                )
            if member.flag_bits & 1:
                raise ValueError(f"encrypted archive member in {path.name}: {member.filename!r}")
            unix_type = (member.external_attr >> 16) & 0o170000
            if member.create_system == 3 and unix_type not in (0, stat.S_IFREG):
                raise ValueError(
                    f"unsupported non-file member in {path.name}: {member.filename!r}"
                )
            if member.file_size > MAX_ARCHIVE_ENTRY_BYTES:
                raise ValueError(
                    f"oversized archive member in {path.name}: {member.filename!r}"
                )
            total_size += member.file_size
            if total_size > MAX_ARCHIVE_CONTENT_BYTES:
                raise ValueError(f"archive expands beyond 256 MiB: {path.name}")
            entries[member.filename] = archive.read(member)
        return entries


def validate_native_archives(directory: Path, version: str) -> None:
    for target in RELEASE_TARGETS:
        name = f"api-subway-{version}-{target.rust}.{target.native_extension}"
        path = directory / name
        entries = zip_entries(path) if target.native_extension == "zip" else tar_entries(path)
        executable = "api-subway.exe" if target.native_extension == "zip" else "api-subway"
        root = f"api-subway-{version}-{target.rust}"
        expected = {
            f"{root}/{executable}",
            f"{root}/LICENSE",
            f"{root}/README.md",
        }
        if set(entries) != expected:
            raise ValueError(f"unexpected contents in {name}")
        if not entries[f"{root}/{executable}"]:
            raise ValueError(f"native executable is empty in {name}")


def validate_npm_packages(directory: Path, version: str) -> None:
    for target in RELEASE_TARGETS:
        name = f"api-subway-{target.npm}-{version}.tgz"
        entries = tar_entries(directory / name)
        executable = "api-subway.exe" if target.npm == "win32-x64" else "api-subway"
        required = {
            "package/package.json",
            f"package/bin/{executable}",
            "package/README.md",
            "package/LICENSE",
        }
        if set(entries) != required:
            raise ValueError(f"unexpected contents in {name}")
        manifest = json.loads(entries["package/package.json"])
        if manifest.get("name") != f"@api-subway/{target.npm}":
            raise ValueError(f"incorrect package name in {name}")
        if manifest.get("version") != version:
            raise ValueError(f"incorrect package version in {name}")
        expected_os, expected_cpu = target.npm.split("-", maxsplit=1)
        if manifest.get("os") != [expected_os] or manifest.get("cpu") != [expected_cpu]:
            raise ValueError(f"incorrect platform constraints in {name}")
        if manifest.get("files") != [f"bin/{executable}"]:
            raise ValueError(f"incorrect package file list in {name}")
        if manifest.get("preferUnplugged") is not True:
            raise ValueError(f"platform package must prefer unplugged installs in {name}")
        expected_libc = ["glibc"] if expected_os == "linux" else None
        if manifest.get("libc") != expected_libc and (
            expected_libc is not None or "libc" in manifest
        ):
            raise ValueError(f"incorrect libc constraint in {name}")

    launcher_name = f"api-subway-{version}.tgz"
    launcher = tar_entries(directory / launcher_name)
    required_launcher = {
        "package/package.json",
        "package/bin/api-subway.js",
        "package/README.md",
        "package/LICENSE",
    }
    if set(launcher) != required_launcher:
        raise ValueError(f"unexpected contents in {launcher_name}")
    launcher_manifest = json.loads(launcher["package/package.json"])
    if launcher_manifest.get("name") != "api-subway":
        raise ValueError(f"incorrect package name in {launcher_name}")
    if launcher_manifest.get("version") != version:
        raise ValueError(f"incorrect package version in {launcher_name}")
    if launcher_manifest.get("bin") != {"api-subway": "bin/api-subway.js"}:
        raise ValueError(f"incorrect launcher binary in {launcher_name}")
    expected_optional = {
        f"@api-subway/{target.npm}": version for target in RELEASE_TARGETS
    }
    if launcher_manifest.get("optionalDependencies") != expected_optional:
        raise ValueError(f"incorrect optional dependencies in {launcher_name}")


def validate_wheels(directory: Path, version: str) -> None:
    for target in RELEASE_TARGETS:
        name = f"api_subway-{version}-py3-none-{target.wheel}.whl"
        entries = zip_entries(directory / name)
        dist_info = f"api_subway-{version}.dist-info"
        executable = "api-subway.exe" if target.npm == "win32-x64" else "api-subway"
        required = {
            "api_subway/__init__.py",
            "api_subway/__main__.py",
            "api_subway/cli.py",
            f"api_subway/bin/{executable}",
            f"{dist_info}/METADATA",
            f"{dist_info}/WHEEL",
            f"{dist_info}/entry_points.txt",
            f"{dist_info}/licenses/LICENSE",
            f"{dist_info}/RECORD",
        }
        if set(entries) != required:
            raise ValueError(f"unexpected contents in {name}")
        metadata = entries[f"{dist_info}/METADATA"].decode("utf-8")
        if f"\nVersion: {version}\n" not in f"\n{metadata}":
            raise ValueError(f"incorrect wheel version in {name}")
        wheel = entries[f"{dist_info}/WHEEL"].decode("utf-8")
        if f"\nTag: py3-none-{target.wheel}\n" not in f"\n{wheel}":
            raise ValueError(f"incorrect wheel tag in {name}")
        validate_wheel_record(name, entries, f"{dist_info}/RECORD")


def validate_wheel_record(
    archive: str, entries: dict[str, bytes], record_path: str
) -> None:
    rows = list(csv.reader(io.StringIO(entries[record_path].decode("utf-8"))))
    if len(rows) != len(entries):
        raise ValueError(f"wheel RECORD entry count is incorrect in {archive}")
    recorded_names: set[str] = set()
    for row in rows:
        if len(row) != 3:
            raise ValueError(f"wheel RECORD row is malformed in {archive}")
        name, digest, size = row
        if name in recorded_names or name not in entries:
            raise ValueError(f"wheel RECORD path is invalid in {archive}: {name!r}")
        recorded_names.add(name)
        if name == record_path:
            if digest or size:
                raise ValueError(f"wheel RECORD must not hash itself in {archive}")
            continue
        expected_digest = base64.urlsafe_b64encode(
            hashlib.sha256(entries[name]).digest()
        ).rstrip(b"=").decode("ascii")
        if digest != f"sha256={expected_digest}" or size != str(len(entries[name])):
            raise ValueError(f"wheel RECORD checksum is incorrect in {archive}: {name!r}")
    if recorded_names != set(entries):
        raise ValueError(f"wheel RECORD is incomplete in {archive}")


def validate_sbom(directory: Path, version: str) -> None:
    name = f"api-subway-{version}.cdx.json"
    payload = json.loads((directory / name).read_text(encoding="utf-8"))
    if payload.get("bomFormat") != "CycloneDX" or payload.get("specVersion") != "1.5":
        raise ValueError(f"unsupported SBOM format in {name}")
    component = payload.get("metadata", {}).get("component", {})
    if component.get("name") != "api-subway" or component.get("version") != version:
        raise ValueError(f"incorrect root component in {name}")
    components = payload.get("components")
    if not isinstance(components, list) or not components:
        raise ValueError(f"SBOM has no components: {name}")
    references = [item.get("bom-ref") for item in components if isinstance(item, dict)]
    if (
        len(references) != len(components)
        or any(not isinstance(reference, str) or not reference for reference in references)
        or len(set(references)) != len(references)
    ):
        raise ValueError(f"SBOM component references are missing or duplicated: {name}")
    root_ref = component.get("bom-ref")
    if not isinstance(root_ref, str) or not root_ref:
        raise ValueError(f"SBOM root reference is missing: {name}")
    known_references = {*references, root_ref}
    dependencies = payload.get("dependencies")
    if not isinstance(dependencies, list) or not dependencies:
        raise ValueError(f"SBOM dependency graph is missing: {name}")
    dependency_refs: set[str] = set()
    for entry in dependencies:
        if not isinstance(entry, dict):
            raise ValueError(f"SBOM dependency entry is malformed: {name}")
        reference = entry.get("ref")
        children = entry.get("dependsOn")
        if (
            not isinstance(reference, str)
            or not reference
            or reference not in known_references
            or reference in dependency_refs
            or not isinstance(children, list)
            or any(
                not isinstance(child, str) or child not in known_references
                for child in children
            )
            or len(children) != len(set(children))
        ):
            raise ValueError(f"SBOM dependency graph is inconsistent: {name}")
        dependency_refs.add(reference)
    if dependency_refs != known_references:
        raise ValueError(f"SBOM dependency graph is incomplete: {name}")


def main() -> int:
    args = parse_args()
    validate_repository_version(args.version)
    directory = args.directory.resolve()
    if not directory.is_dir():
        raise FileNotFoundError(f"release directory does not exist: {directory}")
    verify_release_bundle(directory, args.version)
    validate_native_archives(directory, args.version)
    validate_npm_packages(directory, args.version)
    validate_wheels(directory, args.version)
    validate_sbom(directory, args.version)
    print(f"verified release bundle: {directory}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
