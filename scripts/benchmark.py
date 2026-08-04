#!/usr/bin/env python3
"""Run reproducible local performance benchmarks against a synthetic Next.js API."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/api-subway"),
        help="release binary to benchmark",
    )
    parser.add_argument("--files", type=int, default=1_000)
    parser.add_argument("--lines-per-file", type=int, default=100)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--warmups", type=int, default=1)
    return parser.parse_args()


def validate_args(args: argparse.Namespace) -> None:
    for name in ("files", "rounds"):
        if getattr(args, name) < 1:
            raise ValueError(f"--{name.replace('_', '-')} must be at least 1")
    if args.lines_per_file < 10:
        raise ValueError("--lines-per-file must be at least 10")
    if args.warmups < 0:
        raise ValueError("--warmups cannot be negative")


def route_source(index: int, lines_per_file: int) -> str:
    fixed_lines = [
        f'const routeId = "route-{index}";',
        "",
        "export async function GET() {",
        "  return Response.json({ routeId });",
        "}",
    ]
    filler_count = lines_per_file - len(fixed_lines) - 4
    filler = [f"  {value}," for value in range(filler_count)]
    return "\n".join(
        [*fixed_lines, "", "export const fixtureValues = [", *filler, "] as const;", ""]
    )


def create_fixture(root: Path, files: int, lines_per_file: int) -> None:
    (root / "package.json").write_text(
        '{"private":true,"dependencies":{"next":"16.0.0"}}\n', encoding="utf-8"
    )
    routes = root / "src" / "app" / "api" / "benchmark"
    for index in range(files):
        route_directory = routes / str(index)
        route_directory.mkdir(parents=True)
        (route_directory / "route.ts").write_text(
            route_source(index, lines_per_file), encoding="utf-8"
        )


def run_once(binary: Path, root: Path) -> float:
    output = root / "artifacts" / "api-subway"
    output.parent.mkdir(exist_ok=True)
    started = time.perf_counter()
    process = subprocess.run(
        [
            str(binary),
            "generate",
            str(root),
            "--framework",
            "next",
            "--format",
            "json",
            "--out",
            str(output),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    elapsed = time.perf_counter() - started
    if process.returncode != 0:
        raise RuntimeError(
            f"benchmark command exited with {process.returncode}: {process.stderr.strip()}"
        )
    return elapsed


def main() -> int:
    args = parse_args()
    validate_args(args)
    binary = args.binary.resolve()
    if not binary.is_file():
        raise FileNotFoundError(f"release binary does not exist: {binary}")

    with tempfile.TemporaryDirectory(prefix="api-subway-benchmark-") as directory:
        root = Path(directory)
        fixture_started = time.perf_counter()
        create_fixture(root, args.files, args.lines_per_file)
        fixture_seconds = time.perf_counter() - fixture_started

        for _ in range(args.warmups):
            run_once(binary, root)
        samples = [run_once(binary, root) for _ in range(args.rounds)]

    result = {
        "files": args.files,
        "approximate_loc": args.files * args.lines_per_file,
        "rounds": args.rounds,
        "fixture_seconds": round(fixture_seconds, 4),
        "min_seconds": round(min(samples), 4),
        "median_seconds": round(statistics.median(samples), 4),
        "max_seconds": round(max(samples), 4),
        "files_per_second": round(args.files / statistics.median(samples), 1),
    }
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
