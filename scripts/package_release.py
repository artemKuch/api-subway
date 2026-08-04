#!/usr/bin/env python3
"""Build deterministic native archives, npm packages, and Python wheels."""

from __future__ import annotations

import argparse
import base64
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import zipfile

from release_artifacts import RELEASE_TARGETS, validate_repository_version


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)
MAX_ARCHIVE_ENTRIES = 64
MAX_ARCHIVE_ENTRY_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_CONTENT_BYTES = 256 * 1024 * 1024


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--npm-platform", required=True)
    parser.add_argument("--wheel-platform", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out-dir", type=Path, default=Path("dist"))
    return parser.parse_args()


def validate_target_combination(
    target: str, npm_platform: str, wheel_platform: str
) -> None:
    if not any(
        candidate.rust == target
        and candidate.npm == npm_platform
        and candidate.wheel == wheel_platform
        for candidate in RELEASE_TARGETS
    ):
        raise ValueError(
            "target, npm platform, and wheel platform are not a supported release tuple"
        )


def validate_binary(binary: Path, target: str, version: str) -> None:
    windows_target = target.endswith("windows-msvc")
    if (binary.suffix.lower() == ".exe") != windows_target:
        raise ValueError(f"binary extension does not match release target {target}")
    result = subprocess.run(
        [str(binary), "--version"],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    expected = f"api-subway {version}"
    if result.returncode != 0 or result.stdout.strip() != expected:
        raise ValueError(
            f"native binary version check failed: expected {expected!r}, "
            f"got exit {result.returncode} and {result.stdout.strip()!r}"
        )


def tar_gzip(entries: dict[str, tuple[bytes, int]]) -> bytes:
    validate_archive_entries(entries)
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", mtime=0) as gzip_file:
        with tarfile.open(fileobj=gzip_file, mode="w") as archive:
            for name, (contents, mode) in sorted(entries.items()):
                info = tarfile.TarInfo(name)
                info.size = len(contents)
                info.mode = mode
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                archive.addfile(info, io.BytesIO(contents))
    return compressed.getvalue()


def write_zip(path: Path, entries: dict[str, tuple[bytes, int]]) -> None:
    validate_archive_entries(entries)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, (contents, mode) in sorted(entries.items()):
            info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
            info.create_system = 3
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, contents)


def build_native_archive(
    binary: Path, target: str, version: str, out_dir: Path
) -> Path:
    windows_target = target.endswith("windows-msvc")
    executable = "api-subway.exe" if windows_target else "api-subway"
    root = f"api-subway-{version}-{target}"
    entries = {
        f"{root}/{executable}": (binary.read_bytes(), 0o755),
        f"{root}/LICENSE": ((REPOSITORY_ROOT / "LICENSE").read_bytes(), 0o644),
        f"{root}/README.md": ((REPOSITORY_ROOT / "README.md").read_bytes(), 0o644),
    }
    if windows_target:
        output = out_dir / f"{root}.zip"
        write_zip(output, entries)
    else:
        output = out_dir / f"{root}.tar.gz"
        output.write_bytes(tar_gzip(entries))
    return output


def build_npm_package(
    binary: Path, npm_platform: str, version: str, out_dir: Path
) -> Path:
    package_root = REPOSITORY_ROOT / "packages/npm/platforms" / npm_platform
    manifest_path = package_root / "package.json"
    if not manifest_path.is_file():
        raise ValueError(f"unknown npm platform: {npm_platform}")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("version") != version:
        raise ValueError(
            f"{manifest_path} has version {manifest.get('version')!r}, expected {version!r}"
        )
    executable = "api-subway.exe" if npm_platform == "win32-x64" else "api-subway"
    entries = {
        "package/package.json": (
            (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode(),
            0o644,
        ),
        f"package/bin/{executable}": (binary.read_bytes(), 0o755),
        "package/README.md": (
            (REPOSITORY_ROOT / "packages/npm/README.md").read_bytes(),
            0o644,
        ),
        "package/LICENSE": ((REPOSITORY_ROOT / "LICENSE").read_bytes(), 0o644),
    }
    output = out_dir / f"api-subway-{npm_platform}-{version}.tgz"
    output.write_bytes(tar_gzip(entries))
    return output


def record_hash(contents: bytes) -> str:
    digest = hashlib.sha256(contents).digest()
    encoded = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    return f"sha256={encoded}"


def build_python_wheel(
    binary: Path, wheel_platform: str, version: str, out_dir: Path
) -> Path:
    package_root = REPOSITORY_ROOT / "packages/python/api_subway"
    dist_info = f"api_subway-{version}.dist-info"
    executable = "api-subway.exe" if wheel_platform == "win_amd64" else "api-subway"
    tag = f"py3-none-{wheel_platform}"
    entries: dict[str, tuple[bytes, int]] = {}
    for source in sorted(package_root.glob("*.py")):
        entries[f"api_subway/{source.name}"] = (source.read_bytes(), 0o644)
    entries[f"api_subway/bin/{executable}"] = (binary.read_bytes(), 0o755)
    entries[f"{dist_info}/METADATA"] = (
        (
            "Metadata-Version: 2.4\n"
            "Name: api-subway\n"
            f"Version: {version}\n"
            "Summary: Generate trustworthy API maps in the visual language of a subway map\n"
            "License-Expression: MIT\n"
            "License-File: LICENSE\n"
            "Requires-Python: >=3.9\n"
            "Project-URL: Repository, https://github.com/api-subway/api-subway\n"
            "\n"
        ).encode(),
        0o644,
    )
    entries[f"{dist_info}/WHEEL"] = (
        (
            "Wheel-Version: 1.0\n"
            "Generator: api-subway release packager\n"
            "Root-Is-Purelib: false\n"
            f"Tag: {tag}\n"
            "\n"
        ).encode(),
        0o644,
    )
    entries[f"{dist_info}/entry_points.txt"] = (
        b"[console_scripts]\napi-subway = api_subway.cli:main\n",
        0o644,
    )
    entries[f"{dist_info}/licenses/LICENSE"] = (
        (REPOSITORY_ROOT / "LICENSE").read_bytes(),
        0o644,
    )

    record_path = f"{dist_info}/RECORD"
    record_buffer = io.StringIO(newline="")
    writer = csv.writer(record_buffer, lineterminator="\n")
    for name, (contents, _) in sorted(entries.items()):
        writer.writerow((name, record_hash(contents), len(contents)))
    writer.writerow((record_path, "", ""))
    entries[record_path] = (record_buffer.getvalue().encode(), 0o644)

    output = out_dir / f"api_subway-{version}-{tag}.whl"
    write_zip(output, entries)
    return output


def validate_archive_entries(entries: dict[str, tuple[bytes, int]]) -> None:
    if not entries or len(entries) > MAX_ARCHIVE_ENTRIES:
        raise ValueError(
            f"release archive must contain between 1 and {MAX_ARCHIVE_ENTRIES} files"
        )
    total_size = 0
    for name, (contents, mode) in entries.items():
        path = PurePosixPath(name)
        if (
            not name
            or path.is_absolute()
            or path == PurePosixPath(".")
            or ".." in path.parts
            or "\\" in name
        ):
            raise ValueError(f"unsafe release archive path: {name!r}")
        if mode not in (0o644, 0o755):
            raise ValueError(f"unsupported release archive mode for {name!r}")
        if len(contents) > MAX_ARCHIVE_ENTRY_BYTES:
            raise ValueError(f"release archive entry exceeds 64 MiB: {name!r}")
        total_size += len(contents)
        if total_size > MAX_ARCHIVE_CONTENT_BYTES:
            raise ValueError("release archive contents exceed 256 MiB")


def main() -> int:
    args = parse_args()
    validate_repository_version(args.version)
    validate_target_combination(args.target, args.npm_platform, args.wheel_platform)
    binary = args.binary.resolve()
    if not binary.is_file():
        raise FileNotFoundError(f"native binary does not exist: {binary}")
    validate_binary(binary, args.target, args.version)
    out_dir = args.out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    outputs = [
        build_native_archive(binary, args.target, args.version, out_dir),
        build_npm_package(binary, args.npm_platform, args.version, out_dir),
        build_python_wheel(binary, args.wheel_platform, args.version, out_dir),
    ]
    for output in outputs:
        print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
