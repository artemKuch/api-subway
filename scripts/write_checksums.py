#!/usr/bin/env python3
"""Write or verify a deterministic SHA-256 manifest for a flat artifact directory."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import stat
import tempfile

from release_artifacts import (
    CHECKSUMS_FILE,
    parse_checksums,
    sha256,
    validate_artifact_name,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--output", default=CHECKSUMS_FILE)
    parser.add_argument("--verify", action="store_true")
    return parser.parse_args()


def artifact_files(directory: Path, output_name: str) -> list[Path]:
    validate_artifact_name(output_name)
    files: list[Path] = []
    for path in directory.iterdir():
        if path.name == output_name:
            continue
        validate_artifact_name(path.name)
        if not stat.S_ISREG(path.lstat().st_mode):
            raise ValueError(f"artifact must be a regular file: {path.name}")
        files.append(path)
    files.sort()
    if not files:
        raise ValueError(f"no artifacts found in {directory}")
    return files


def write_manifest(directory: Path, output_name: str) -> Path:
    files = artifact_files(directory, output_name)
    output = directory / output_name
    contents = "".join(f"{sha256(path)}  {path.name}\n" for path in files)
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            dir=directory,
            prefix=f".{output.name}.",
            encoding="utf-8",
            newline="\n",
            delete=False,
        ) as temporary:
            temporary.write(contents)
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary_name = temporary.name
        Path(temporary_name).replace(output)
    finally:
        if temporary_name is not None:
            Path(temporary_name).unlink(missing_ok=True)
    return output


def verify_manifest(directory: Path, output_name: str) -> None:
    checksums = parse_checksums(directory / output_name)
    actual_names = {path.name for path in artifact_files(directory, output_name)}
    if set(checksums) != actual_names:
        missing = sorted(actual_names - set(checksums))
        unexpected = sorted(set(checksums) - actual_names)
        details: list[str] = []
        if missing:
            details.append(f"missing entries: {', '.join(missing)}")
        if unexpected:
            details.append(f"unknown entries: {', '.join(unexpected)}")
        raise ValueError("checksum manifest does not match directory; " + "; ".join(details))
    for name, expected in sorted(checksums.items()):
        if sha256(directory / name) != expected:
            raise ValueError(f"checksum mismatch for {name}")


def main() -> int:
    args = parse_args()
    directory = args.directory.resolve()
    if not directory.is_dir():
        raise FileNotFoundError(f"artifact directory does not exist: {directory}")
    if args.verify:
        verify_manifest(directory, args.output)
        print(directory / args.output)
    else:
        print(write_manifest(directory, args.output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
