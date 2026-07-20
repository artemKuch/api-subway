#!/usr/bin/env python3
"""Verify that a release is fully visible in every public package registry."""

from __future__ import annotations

import argparse
import json
import time
from collections.abc import Callable
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

from release_artifacts import RELEASE_TARGETS, validate_stable_release_version


MAX_RESPONSE_BYTES = 8 * 1024 * 1024
USER_AGENT = "api-subway-release/0.1 (https://github.com/artemKuch/api-subway)"
NPM_PACKAGES = (
    *(f"@api-subway/{target.npm}" for target in RELEASE_TARGETS),
    "api-subway",
)
CARGO_CRATES = (
    "api-subway-core",
    "api-subway-analyzers",
    "api-subway-renderer",
    "api-subway",
)

JsonObject = dict[str, object]
JsonFetcher = Callable[[str], JsonObject | None]


class RegistryRequestError(RuntimeError):
    """Raised when a registry response cannot be verified."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--poll-seconds", type=int, default=10)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    return parser.parse_args()


def npm_version_url(package: str, version: str) -> str:
    return (
        "https://registry.npmjs.org/"
        f"{quote(package, safe='')}/{quote(version, safe='')}"
    )


def pypi_version_url(version: str) -> str:
    return f"https://pypi.org/pypi/api-subway/{quote(version, safe='')}/json"


def crate_version_url(crate: str, version: str) -> str:
    return (
        "https://crates.io/api/v1/crates/"
        f"{quote(crate, safe='')}/{quote(version, safe='')}"
    )


def fetch_json(url: str) -> JsonObject | None:
    request = Request(
        url,
        headers={"Accept": "application/json", "User-Agent": USER_AGENT},
    )
    try:
        with urlopen(request, timeout=30) as response:
            contents = response.read(MAX_RESPONSE_BYTES + 1)
    except HTTPError as error:
        if error.code == 404:
            return None
        raise RegistryRequestError(f"HTTP {error.code} from {url}") from error
    except URLError as error:
        raise RegistryRequestError(
            f"request failed for {url}: {error.reason}"
        ) from error

    if len(contents) > MAX_RESPONSE_BYTES:
        raise RegistryRequestError(f"response exceeds 8 MiB: {url}")
    try:
        payload = json.loads(contents)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RegistryRequestError(f"invalid JSON from {url}") from error
    if not isinstance(payload, dict):
        raise RegistryRequestError(f"expected a JSON object from {url}")
    return payload


def _load(
    label: str,
    url: str,
    fetcher: JsonFetcher,
    issues: list[str],
) -> JsonObject | None:
    try:
        payload = fetcher(url)
    except RegistryRequestError as error:
        issues.append(f"{label}: {error}")
        return None
    if payload is None:
        issues.append(f"{label}: not found")
    return payload


def publication_issues(
    version: str,
    fetcher: JsonFetcher = fetch_json,
) -> list[str]:
    validate_stable_release_version(version)
    issues: list[str] = []

    for package in NPM_PACKAGES:
        label = f"npm {package}@{version}"
        payload = _load(label, npm_version_url(package, version), fetcher, issues)
        if payload is None:
            continue
        distribution = payload.get("dist")
        if payload.get("name") != package or payload.get("version") != version:
            issues.append(f"{label}: metadata mismatch")
        elif not isinstance(distribution, dict) or not distribution.get("integrity"):
            issues.append(f"{label}: missing package integrity")

    pypi_label = f"PyPI api-subway=={version}"
    pypi = _load(pypi_label, pypi_version_url(version), fetcher, issues)
    if pypi is not None:
        expected_wheels = {
            f"api_subway-{version}-py3-none-{target.wheel}.whl"
            for target in RELEASE_TARGETS
        }
        urls = pypi.get("urls")
        files = (
            {item.get("filename") for item in urls if isinstance(item, dict)}
            if isinstance(urls, list)
            else set()
        )
        info = pypi.get("info")
        if not isinstance(info, dict) or info.get("version") != version:
            issues.append(f"{pypi_label}: metadata mismatch")
        elif files != expected_wheels:
            issues.append(f"{pypi_label}: wheel set is incomplete")

    for crate in CARGO_CRATES:
        label = f"crates.io {crate} {version}"
        payload = _load(label, crate_version_url(crate, version), fetcher, issues)
        if payload is None:
            continue
        version_metadata = payload.get("version")
        if (
            not isinstance(version_metadata, dict)
            or version_metadata.get("crate") != crate
            or version_metadata.get("num") != version
        ):
            issues.append(f"{label}: metadata mismatch")

    return issues


def wait_for_publication(
    version: str,
    poll_seconds: int,
    timeout_seconds: int,
) -> None:
    if poll_seconds < 1 or timeout_seconds < poll_seconds:
        raise ValueError("poll and timeout values must be positive and timeout >= poll")
    deadline = time.monotonic() + timeout_seconds
    while True:
        issues = publication_issues(version)
        if not issues:
            print(f"api-subway {version} is available in npm, PyPI, and crates.io")
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("registry verification timed out: " + "; ".join(issues))
        print("Waiting for registry propagation: " + "; ".join(issues))
        time.sleep(min(poll_seconds, remaining))


def main() -> int:
    args = parse_args()
    validate_stable_release_version(args.version)
    wait_for_publication(
        args.version,
        poll_seconds=args.poll_seconds,
        timeout_seconds=args.timeout_seconds,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
