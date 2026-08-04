#!/usr/bin/env python3
"""Validate committed schemas and representative generated documents."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
MAX_JSON_FILE_BYTES = 64 * 1024 * 1024
MAX_BACKEND_DOCUMENT_BYTES = 2_000_000
MAX_BACKEND_JSON_NODES = 100_000
MAX_BACKEND_JSON_DEPTH = 20


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPOSITORY_ROOT)
    return parser.parse_args()


def load_json(path: Path) -> object:
    with path.open("rb") as source:
        contents = source.read(MAX_JSON_FILE_BYTES + 1)
    if len(contents) > MAX_JSON_FILE_BYTES:
        raise ValueError(f"{path.name} exceeds the 64 MiB validation budget")
    return json.loads(contents, object_pairs_hook=reject_duplicate_keys)


def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON object key: {key!r}")
        value[key] = item
    return value


def validate_document(schema_path: Path, document_path: Path) -> None:
    from jsonschema import FormatChecker
    from jsonschema.validators import validator_for

    schema = load_json(schema_path)
    validator_type = validator_for(schema)
    validator_type.check_schema(schema)
    validator = validator_type(schema, format_checker=FormatChecker())
    errors = sorted(
        validator.iter_errors(load_json(document_path)),
        key=lambda error: tuple(str(part) for part in error.absolute_path),
    )
    if errors:
        formatted = []
        for error in errors:
            location = "/".join(str(part) for part in error.absolute_path) or "<root>"
            formatted.append(f"{document_path.name}:{location}: {error.message}")
        raise ValueError("\n".join(formatted))


def validate_virtual_backend_runtime(
    schema_path: Path,
    document_path: Path,
) -> None:
    if document_path.stat().st_size > MAX_BACKEND_DOCUMENT_BYTES:
        raise ValueError(f"{document_path.name} exceeds the 2 MB runtime budget")
    schema = load_json(schema_path)
    if not isinstance(schema, dict):
        raise ValueError("virtual backend schema must be an object")
    declared = schema.get("x-api-subway-runtime")
    expected = {
        "maxDocumentBytes": MAX_BACKEND_DOCUMENT_BYTES,
        "maxRecordJsonNodes": MAX_BACKEND_JSON_NODES,
        "maxRecordJsonDepth": MAX_BACKEND_JSON_DEPTH,
        "primaryKey": {
            "requiredOnEveryRecord": True,
            "allowedTypes": ["string", "number"],
            "uniqueWithinResource": True,
        },
    }
    if declared != expected:
        raise ValueError("virtual backend runtime limits are not synchronized")
    document = load_json(document_path)
    if not isinstance(document, dict) or not isinstance(document.get("resources"), dict):
        raise ValueError("virtual backend document has no resources object")
    record_node_budget = {"remaining": MAX_BACKEND_JSON_NODES}
    for resource_name, resource in document["resources"].items():
        if not isinstance(resource, dict):
            raise ValueError(f"resource {resource_name!r} must be an object")
        primary_key = resource.get("primaryKey")
        records = resource.get("records")
        if not isinstance(primary_key, str) or not isinstance(records, list):
            raise ValueError(f"resource {resource_name!r} is malformed")
        identities: set[str] = set()
        for index, record in enumerate(records):
            assert_bounded_json(record, record_node_budget)
            if not isinstance(record, dict) or primary_key not in record:
                raise ValueError(
                    f"resource {resource_name!r} record {index} has no {primary_key!r}"
                )
            identity = runtime_identity(record[primary_key])
            if identity in identities:
                raise ValueError(
                    f"resource {resource_name!r} contains duplicate {primary_key!r}"
                )
            identities.add(identity)


def assert_bounded_json(value: object, budget: dict[str, int]) -> None:
    stack = [(value, 0)]
    while stack:
        current, depth = stack.pop()
        budget["remaining"] -= 1
        if budget["remaining"] < 0:
            raise ValueError("virtual backend exceeds the aggregate JSON-node budget")
        if depth > MAX_BACKEND_JSON_DEPTH:
            raise ValueError("virtual backend exceeds the JSON-depth budget")
        if isinstance(current, dict):
            stack.extend((item, depth + 1) for item in current.values())
        elif isinstance(current, list):
            stack.extend((item, depth + 1) for item in current)


def runtime_identity(value: object) -> str:
    if isinstance(value, str):
        if not value:
            raise ValueError("virtual backend primary keys cannot be empty")
        return value
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError("virtual backend primary keys must be strings or numbers")
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("virtual backend numeric primary keys must be finite")
    if number.is_integer():
        return str(int(number))
    return format(number, ".15g")


def main() -> int:
    root = parse_args().root.resolve()
    api_schema = root / "schemas/api-subway-v1.schema.json"
    backend_schema = root / "schemas/virtual-backend-v1.schema.json"
    for name in ("map-10.json", "map-40.json", "map-100.json"):
        validate_document(api_schema, root / "fixtures/golden" / name)
    validate_document(api_schema, root / "docs/api-subway.json")
    validate_document(
        backend_schema,
        root / "fixtures/virtual-backend/store.json",
    )
    validate_virtual_backend_runtime(
        backend_schema,
        root / "fixtures/virtual-backend/store.json",
    )
    print("validated JSON Schemas and representative documents")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
