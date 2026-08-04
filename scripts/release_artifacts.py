#!/usr/bin/env python3
"""Shared release artifact naming, version, and integrity rules."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import stat
import tomllib


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
CHECKSUMS_FILE = "SHA256SUMS"
MAX_ARTIFACT_BYTES = 256 * 1024 * 1024
MAX_CHECKSUM_BYTES = 1024 * 1024
MAX_CHECKSUM_ENTRIES = 1_000
SEMVER = re.compile(
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)"
    r"(?:-(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)
STABLE_SEMVER = re.compile(
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)"
)


@dataclass(frozen=True)
class ReleaseTarget:
    rust: str
    npm: str
    wheel: str
    native_extension: str


RELEASE_TARGETS = (
    ReleaseTarget(
        rust="x86_64-unknown-linux-gnu",
        npm="linux-x64",
        wheel="manylinux_2_35_x86_64",
        native_extension="tar.gz",
    ),
    ReleaseTarget(
        rust="aarch64-unknown-linux-gnu",
        npm="linux-arm64",
        wheel="manylinux_2_35_aarch64",
        native_extension="tar.gz",
    ),
    ReleaseTarget(
        rust="x86_64-apple-darwin",
        npm="darwin-x64",
        wheel="macosx_11_0_x86_64",
        native_extension="tar.gz",
    ),
    ReleaseTarget(
        rust="aarch64-apple-darwin",
        npm="darwin-arm64",
        wheel="macosx_11_0_arm64",
        native_extension="tar.gz",
    ),
    ReleaseTarget(
        rust="x86_64-pc-windows-msvc",
        npm="win32-x64",
        wheel="win_amd64",
        native_extension="zip",
    ),
)


def validate_semver(version: str) -> None:
    if not SEMVER.fullmatch(version):
        raise ValueError(f"invalid semantic version: {version!r}")


def validate_stable_release_version(version: str) -> None:
    if not STABLE_SEMVER.fullmatch(version):
        raise ValueError(
            "production releases require a stable X.Y.Z version; "
            f"got {version!r}"
        )


def validate_artifact_name(name: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,254}", name):
        raise ValueError(f"invalid flat artifact file name: {name!r}")


def _json_version(path: Path) -> str | None:
    payload = json.loads(path.read_text(encoding="utf-8"))
    version = payload.get("version")
    return version if isinstance(version, str) else None


def repository_versions(root: Path = REPOSITORY_ROOT) -> dict[str, str | None]:
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    python_project = tomllib.loads(
        (root / "packages/python/pyproject.toml").read_text(encoding="utf-8")
    )
    python_init = (
        root / "packages/python/api_subway/__init__.py"
    ).read_text(encoding="utf-8")
    init_match = re.search(
        r'^__version__\s*=\s*"([^"]+)"', python_init, flags=re.MULTILINE
    )

    versions: dict[str, str | None] = {
        "Cargo workspace": cargo["workspace"]["package"].get("version"),
        "npm launcher": _json_version(root / "packages/npm/package.json"),
        "Python project": python_project["project"].get("version"),
        "Python package": init_match.group(1) if init_match else None,
        "viewer": _json_version(
            root / "crates/api-subway-renderer/viewer/package.json"
        ),
    }
    for target in RELEASE_TARGETS:
        versions[f"npm platform {target.npm}"] = _json_version(
            root / "packages/npm/platforms" / target.npm / "package.json"
        )
    return versions


def validate_repository_version(
    version: str, root: Path = REPOSITORY_ROOT
) -> None:
    validate_stable_release_version(version)
    mismatches = {
        name: found
        for name, found in repository_versions(root).items()
        if found != version
    }
    if mismatches:
        details = ", ".join(
            f"{name}={found!r}" for name, found in sorted(mismatches.items())
        )
        raise ValueError(
            f"release version {version!r} does not match repository metadata: {details}"
        )


def expected_artifact_names(version: str) -> set[str]:
    validate_stable_release_version(version)
    names = {
        f"api-subway-{version}.tgz",
        f"api-subway-{version}.cdx.json",
        CHECKSUMS_FILE,
    }
    for target in RELEASE_TARGETS:
        names.add(
            f"api-subway-{version}-{target.rust}.{target.native_extension}"
        )
        names.add(f"api-subway-{target.npm}-{version}.tgz")
        names.add(f"api_subway-{version}-py3-none-{target.wheel}.whl")
    return names


def checksum_subject_names(version: str) -> set[str]:
    return expected_artifact_names(version) - {CHECKSUMS_FILE}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_checksums(path: Path) -> dict[str, str]:
    status = path.lstat()
    if not stat.S_ISREG(status.st_mode):
        raise ValueError(f"checksum manifest must be a regular file: {path.name}")
    with path.open("rb") as source:
        contents = source.read(MAX_CHECKSUM_BYTES + 1)
    if len(contents) > MAX_CHECKSUM_BYTES:
        raise ValueError("checksum manifest exceeds 1 MiB")
    try:
        lines = contents.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ValueError("checksum manifest is not valid UTF-8") from error
    if len(lines) > MAX_CHECKSUM_ENTRIES:
        raise ValueError(
            f"checksum manifest exceeds {MAX_CHECKSUM_ENTRIES} entries"
        )
    checksums: dict[str, str] = {}
    for line_number, raw_line in enumerate(lines, start=1):
        if not raw_line:
            continue
        match = re.fullmatch(r"([0-9a-f]{64}) ([ *])([^/\\]+)", raw_line)
        if match is None:
            raise ValueError(
                f"{path.name}:{line_number}: invalid shasum-compatible entry"
            )
        digest, _, name = match.groups()
        validate_artifact_name(name)
        if name in checksums:
            raise ValueError(f"{path.name}:{line_number}: duplicate entry {name!r}")
        checksums[name] = digest
    if not checksums:
        raise ValueError(f"{path.name} contains no checksums")
    return checksums


def verify_release_bundle(directory: Path, version: str) -> None:
    expected = expected_artifact_names(version)
    entries = list(directory.iterdir())
    for path in entries:
        if not stat.S_ISREG(path.lstat().st_mode):
            raise ValueError(f"release bundle contains a non-regular entry: {path.name}")
    actual = {path.name for path in entries}
    missing = sorted(expected - actual)
    unexpected = sorted(actual - expected)
    if missing or unexpected:
        details: list[str] = []
        if missing:
            details.append(f"missing: {', '.join(missing)}")
        if unexpected:
            details.append(f"unexpected: {', '.join(unexpected)}")
        raise ValueError("invalid release bundle; " + "; ".join(details))

    for name in sorted(expected):
        size = (directory / name).stat().st_size
        if size == 0:
            raise ValueError(f"release artifact is empty: {name}")
        maximum = MAX_CHECKSUM_BYTES if name == CHECKSUMS_FILE else MAX_ARTIFACT_BYTES
        if size > maximum:
            if name == CHECKSUMS_FILE:
                raise ValueError("checksum manifest exceeds 1 MiB")
            raise ValueError(f"release artifact exceeds 256 MiB: {name}")

    checksum_path = directory / CHECKSUMS_FILE
    checksums = parse_checksums(checksum_path)
    expected_subjects = checksum_subject_names(version)
    if set(checksums) != expected_subjects:
        missing_subjects = sorted(expected_subjects - set(checksums))
        extra_subjects = sorted(set(checksums) - expected_subjects)
        details = []
        if missing_subjects:
            details.append(f"missing checksum: {', '.join(missing_subjects)}")
        if extra_subjects:
            details.append(f"unexpected checksum: {', '.join(extra_subjects)}")
        raise ValueError("invalid checksum manifest; " + "; ".join(details))

    for name, expected_digest in sorted(checksums.items()):
        actual_digest = sha256(directory / name)
        if actual_digest != expected_digest:
            raise ValueError(f"checksum mismatch for {name}")
