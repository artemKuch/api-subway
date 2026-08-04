#!/usr/bin/env python3
"""Publish npm release artifacts in dependency order and support safe reruns."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time

from release_artifacts import (
    RELEASE_TARGETS,
    validate_repository_version,
    verify_release_bundle,
)
from verify_release import validate_npm_packages


NPM_REGISTRY = "https://registry.npmjs.org"
COMMAND_TIMEOUT_SECONDS = 300


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    return parser.parse_args()


def published_version(package: str, version: str) -> str | None:
    result = subprocess.run(
        [
            "npm",
            "view",
            f"{package}@{version}",
            "version",
            "--json",
            "--registry",
            NPM_REGISTRY,
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if result.returncode == 0:
        value = json.loads(result.stdout)
        if value != version:
            raise RuntimeError(
                f"npm returned unexpected version for {package}@{version}: {value!r}"
            )
        return value
    if "E404" in result.stderr or "404 Not Found" in result.stderr:
        return None
    raise RuntimeError(
        f"could not query npm for {package}@{version}: {result.stderr.strip()}"
    )


def publish(package: str, version: str, archive: Path) -> None:
    if published_version(package, version) is not None:
        print(f"{package}@{version} is already published")
        return
    subprocess.run(
        [
            "npm",
            "publish",
            str(archive),
            "--access",
            "public",
            "--provenance",
            "--registry",
            NPM_REGISTRY,
        ],
        check=True,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        if published_version(package, version) is not None:
            break
        time.sleep(10)
    else:
        raise TimeoutError(f"{package}@{version} was not visible after 300 seconds")
    print(f"{package}@{version} is available")


def main() -> int:
    args = parse_args()
    validate_repository_version(args.version)
    directory = args.directory.resolve()
    if not directory.is_dir():
        raise FileNotFoundError(f"release directory does not exist: {directory}")
    verify_release_bundle(directory, args.version)
    validate_npm_packages(directory, args.version)
    for target in RELEASE_TARGETS:
        publish(
            f"@api-subway/{target.npm}",
            args.version,
            directory / f"api-subway-{target.npm}-{args.version}.tgz",
        )
    publish(
        "api-subway",
        args.version,
        directory / f"api-subway-{args.version}.tgz",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
