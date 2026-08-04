#!/usr/bin/env python3
"""Publish workspace crates in dependency order with idempotent index polling."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

from release_artifacts import validate_repository_version


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
CRATES = (
    "api-subway-core",
    "api-subway-analyzers",
    "api-subway-renderer",
    "api-subway",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--poll-seconds", type=int, default=10)
    parser.add_argument("--timeout-seconds", type=int, default=300)
    return parser.parse_args()


def version_exists(crate: str, version: str) -> bool:
    url = f"https://crates.io/api/v1/crates/{quote(crate)}/{quote(version)}"
    request = Request(
        url,
        headers={
            "User-Agent": (
                "api-subway-release/0.1 "
                "(https://github.com/api-subway/api-subway)"
            )
        },
    )
    try:
        with urlopen(request, timeout=20) as response:
            payload = json.load(response)
        return payload.get("version", {}).get("num") == version
    except HTTPError as error:
        if error.code == 404:
            return False
        raise
    except URLError as error:
        raise RuntimeError(f"could not query crates.io for {crate}: {error}") from error


def wait_until_available(
    crate: str, version: str, poll_seconds: int, timeout_seconds: int
) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if version_exists(crate, version):
            return
        time.sleep(poll_seconds)
    raise TimeoutError(
        f"{crate} {version} was not visible on crates.io after {timeout_seconds} seconds"
    )


def main() -> int:
    args = parse_args()
    if args.poll_seconds < 1 or args.timeout_seconds < args.poll_seconds:
        raise ValueError("poll and timeout values must be positive and timeout >= poll")
    if not os.environ.get("CARGO_REGISTRY_TOKEN"):
        raise RuntimeError("CARGO_REGISTRY_TOKEN is required")
    validate_repository_version(args.version)

    for crate in CRATES:
        if version_exists(crate, args.version):
            print(f"{crate} {args.version} is already published")
            continue
        subprocess.run(
            [
                "cargo",
                "publish",
                "--locked",
                "--registry",
                "crates-io",
                "--package",
                crate,
            ],
            cwd=REPOSITORY_ROOT,
            check=True,
            timeout=900,
        )
        wait_until_available(
            crate, args.version, args.poll_seconds, args.timeout_seconds
        )
        print(f"{crate} {args.version} is available")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
