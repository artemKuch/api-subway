from __future__ import annotations

import json
from pathlib import Path
import stat
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

import generate_sbom
import package_release
import validate_schemas
import verify_release
import verify_registry_publication
from release_artifacts import (
    CHECKSUMS_FILE,
    RELEASE_TARGETS,
    expected_artifact_names,
    parse_checksums,
    repository_versions,
    sha256,
    validate_repository_version,
    validate_semver,
    validate_stable_release_version,
    verify_release_bundle,
)
from write_checksums import verify_manifest, write_manifest


class ReleaseMetadataTest(unittest.TestCase):
    def test_repository_versions_are_aligned(self) -> None:
        versions = repository_versions()
        self.assertGreaterEqual(len(versions), 10)
        self.assertEqual(set(versions.values()), {"0.1.0"})
        validate_repository_version("0.1.0")

    def test_semver_rejects_ambiguous_versions(self) -> None:
        for invalid in ("1.2", "01.2.3", "1.2.3rc1", "v1.2.3", "1.2.3-"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                validate_semver(invalid)
        validate_semver("1.2.3-rc.1")
        with self.assertRaisesRegex(ValueError, "stable X.Y.Z"):
            validate_stable_release_version("1.2.3-rc.1")

    def test_release_packages_are_byte_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "api-subway"
            binary.write_bytes(b"synthetic-native-binary")
            first = root / "first"
            second = root / "second"
            first.mkdir()
            second.mkdir()

            builders = (
                lambda out: package_release.build_native_archive(
                    binary, "x86_64-unknown-linux-gnu", "0.1.0", out
                ),
                lambda out: package_release.build_npm_package(
                    binary, "linux-x64", "0.1.0", out
                ),
                lambda out: package_release.build_python_wheel(
                    binary, "manylinux_2_35_x86_64", "0.1.0", out
                ),
            )
            for builder in builders:
                left = builder(first)
                right = builder(second)
                self.assertEqual(left.name, right.name)
                self.assertEqual(sha256(left), sha256(right))

    def test_release_target_tuple_must_match(self) -> None:
        package_release.validate_target_combination(
            "x86_64-unknown-linux-gnu",
            "linux-x64",
            "manylinux_2_35_x86_64",
        )
        with self.assertRaisesRegex(ValueError, "supported release tuple"):
            package_release.validate_target_combination(
                "x86_64-unknown-linux-gnu",
                "darwin-x64",
                "manylinux_2_35_x86_64",
            )

    def test_windows_packaging_normalizes_uppercase_executable_suffix(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = directory / "API-SUBWAY.EXE"
            binary.write_bytes(b"windows-binary")
            native = package_release.build_native_archive(
                binary, "x86_64-pc-windows-msvc", "0.1.0", directory
            )
            npm = package_release.build_npm_package(
                binary, "win32-x64", "0.1.0", directory
            )
            wheel = package_release.build_python_wheel(
                binary, "win_amd64", "0.1.0", directory
            )
            self.assertTrue(
                any(
                    name.endswith("/api-subway.exe")
                    for name in verify_release.zip_entries(native)
                )
            )
            self.assertIn("package/bin/api-subway.exe", verify_release.tar_entries(npm))
            self.assertIn("api_subway/bin/api-subway.exe", verify_release.zip_entries(wheel))

    def test_binary_version_must_match_release(self) -> None:
        completed = mock.Mock(returncode=0, stdout="api-subway 9.9.9\n")
        with mock.patch.object(package_release.subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(ValueError, "version check failed"):
                package_release.validate_binary(
                    Path("api-subway"), "x86_64-unknown-linux-gnu", "0.1.0"
                )

    def test_archive_validation_rejects_links_and_duplicate_members(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            linked = directory / "linked.tar.gz"
            with tarfile.open(linked, "w:gz") as archive:
                link = tarfile.TarInfo("api-subway")
                link.type = tarfile.SYMTYPE
                link.linkname = "../../outside"
                archive.addfile(link)
            with self.assertRaisesRegex(ValueError, "non-file member"):
                verify_release.tar_entries(linked)

            duplicate = directory / "duplicate.zip"
            with zipfile.ZipFile(duplicate, "w") as archive:
                with self.assertWarns(UserWarning):
                    archive.writestr("same", b"first")
                    archive.writestr("same", b"second")
            with self.assertRaisesRegex(ValueError, "duplicate member"):
                verify_release.zip_entries(duplicate)

            zip_link = directory / "linked.zip"
            with zipfile.ZipFile(zip_link, "w") as archive:
                link = zipfile.ZipInfo("api-subway")
                link.create_system = 3
                link.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(link, "../../outside")
            with self.assertRaisesRegex(ValueError, "non-file member"):
                verify_release.zip_entries(zip_link)

            crowded = directory / "crowded.tar.gz"
            with tarfile.open(crowded, "w:gz") as archive:
                for index in range(verify_release.MAX_ARCHIVE_MEMBERS + 1):
                    member = tarfile.TarInfo(f"file-{index}")
                    member.size = 0
                    archive.addfile(member)
            with self.assertRaisesRegex(ValueError, "more than"):
                verify_release.tar_entries(crowded)

    def test_wheel_record_tampering_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = directory / "api-subway"
            binary.write_bytes(b"synthetic-native-binary")
            wheel = package_release.build_python_wheel(
                binary, "manylinux_2_35_x86_64", "0.1.0", directory
            )
            entries = verify_release.zip_entries(wheel)
            entries["api_subway/__init__.py"] = b"tampered"
            with self.assertRaisesRegex(ValueError, "checksum is incorrect"):
                verify_release.validate_wheel_record(
                    wheel.name, entries, "api_subway-0.1.0.dist-info/RECORD"
                )

    def test_checksum_manifest_detects_changes_and_unknown_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "one.bin").write_bytes(b"one")
            (directory / "two.bin").write_bytes(b"two")
            manifest = write_manifest(directory, CHECKSUMS_FILE)
            self.assertEqual(sorted(parse_checksums(manifest)), ["one.bin", "two.bin"])
            verify_manifest(directory, CHECKSUMS_FILE)

            (directory / "one.bin").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                verify_manifest(directory, CHECKSUMS_FILE)

            (directory / "three.bin").write_bytes(b"three")
            with self.assertRaisesRegex(ValueError, "does not match directory"):
                verify_manifest(directory, CHECKSUMS_FILE)

    def test_release_bundle_requires_every_expected_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            names = expected_artifact_names("0.1.0") - {CHECKSUMS_FILE}
            for name in names:
                (directory / name).write_bytes(name.encode())
            write_manifest(directory, CHECKSUMS_FILE)
            verify_release_bundle(directory, "0.1.0")

            (directory / sorted(names)[0]).unlink()
            with self.assertRaisesRegex(ValueError, "missing"):
                verify_release_bundle(directory, "0.1.0")

    def test_release_bundle_rejects_symlinked_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            names = expected_artifact_names("0.1.0") - {CHECKSUMS_FILE}
            for name in names:
                (directory / name).write_bytes(name.encode())
            linked_name = sorted(names)[0]
            (directory / linked_name).unlink()
            (directory / linked_name).symlink_to(directory / sorted(names)[1])
            write_manifest_target = directory / CHECKSUMS_FILE
            write_manifest_target.write_text("placeholder", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "non-regular"):
                verify_release_bundle(directory, "0.1.0")

    def test_registry_publication_requires_every_package(self) -> None:
        version = "0.1.0"
        payloads: dict[str, dict[str, object]] = {}
        for package in verify_registry_publication.NPM_PACKAGES:
            payloads[verify_registry_publication.npm_version_url(package, version)] = {
                "name": package,
                "version": version,
                "dist": {"integrity": "sha512-example"},
            }
        payloads[verify_registry_publication.pypi_version_url(version)] = {
            "info": {"version": version},
            "urls": [
                {"filename": f"api_subway-{version}-py3-none-{target.wheel}.whl"}
                for target in RELEASE_TARGETS
            ],
        }
        for crate in verify_registry_publication.CARGO_CRATES:
            payloads[verify_registry_publication.crate_version_url(crate, version)] = {
                "version": {"crate": crate, "num": version},
            }

        self.assertEqual(
            verify_registry_publication.publication_issues(version, payloads.get),
            [],
        )

        missing_url = verify_registry_publication.npm_version_url("api-subway", version)
        del payloads[missing_url]
        self.assertIn(
            "npm api-subway@0.1.0: not found",
            verify_registry_publication.publication_issues(version, payloads.get),
        )

    def test_registry_publication_rejects_incomplete_wheel_set(self) -> None:
        version = "0.1.0"

        def fetcher(url: str) -> dict[str, object] | None:
            if url == verify_registry_publication.pypi_version_url(version):
                return {"info": {"version": version}, "urls": []}
            if "registry.npmjs.org" in url:
                package = next(
                    package
                    for package in verify_registry_publication.NPM_PACKAGES
                    if url
                    == verify_registry_publication.npm_version_url(package, version)
                )
                return {
                    "name": package,
                    "version": version,
                    "dist": {"integrity": "sha512-example"},
                }
            crate = next(
                crate
                for crate in verify_registry_publication.CARGO_CRATES
                if url == verify_registry_publication.crate_version_url(crate, version)
            )
            return {"version": {"crate": crate, "num": version}}

        self.assertIn(
            "PyPI api-subway==0.1.0: wheel set is incomplete",
            verify_registry_publication.publication_issues(version, fetcher),
        )

    def test_complete_synthetic_bundle_passes_archive_validation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "dist"
            inputs = root / "inputs"
            directory.mkdir()
            inputs.mkdir()
            for target in RELEASE_TARGETS:
                executable = "api-subway.exe" if target.native_extension == "zip" else "api-subway"
                binary = inputs / target.npm / executable
                binary.parent.mkdir()
                binary.write_bytes(f"binary:{target.rust}".encode())
                package_release.build_native_archive(
                    binary, target.rust, "0.1.0", directory
                )
                package_release.build_npm_package(
                    binary, target.npm, "0.1.0", directory
                )
                package_release.build_python_wheel(
                    binary, target.wheel, "0.1.0", directory
                )

            launcher = package_release.tar_gzip(
                {
                    "package/package.json": (
                        (SCRIPTS.parent / "packages/npm/package.json").read_bytes(),
                        0o644,
                    ),
                    "package/bin/api-subway.js": (
                        (SCRIPTS.parent / "packages/npm/bin/api-subway.js").read_bytes(),
                        0o755,
                    ),
                    "package/README.md": (
                        (SCRIPTS.parent / "packages/npm/README.md").read_bytes(),
                        0o644,
                    ),
                    "package/LICENSE": (
                        (SCRIPTS.parent / "LICENSE").read_bytes(),
                        0o644,
                    ),
                }
            )
            (directory / "api-subway-0.1.0.tgz").write_bytes(launcher)
            (directory / "api-subway-0.1.0.cdx.json").write_text(
                json.dumps(generate_sbom.build_sbom("0.1.0")), encoding="utf-8"
            )
            write_manifest(directory, CHECKSUMS_FILE)

            verify_release.verify_release_bundle(directory, "0.1.0")
            verify_release.validate_native_archives(directory, "0.1.0")
            verify_release.validate_npm_packages(directory, "0.1.0")
            verify_release.validate_wheels(directory, "0.1.0")
            verify_release.validate_sbom(directory, "0.1.0")


class SbomTest(unittest.TestCase):
    def test_sbom_is_deterministic_and_has_unique_components(self) -> None:
        first = generate_sbom.build_sbom("0.1.0")
        second = generate_sbom.build_sbom("0.1.0")
        self.assertEqual(first, second)
        self.assertEqual(first["bomFormat"], "CycloneDX")
        self.assertEqual(first["specVersion"], "1.5")
        self.assertNotIn("timestamp", json.dumps(first))

        components = first["components"]
        references = [component["bom-ref"] for component in components]
        self.assertEqual(len(references), len(set(references)))
        ecosystems = {
            property_value["value"]
            for component in components
            for property_value in component["properties"]
            if property_value["name"] == "api-subway:ecosystem"
        }
        self.assertEqual(ecosystems, {"cargo", "npm"})

        graph = {
            entry["ref"]: entry["dependsOn"]
            for entry in first["dependencies"]
        }
        self.assertEqual(set(graph), {*references, first["metadata"]["component"]["bom-ref"]})
        esbuild = next(
            component for component in components
            if component["name"] == "esbuild"
            and any(
                item["name"] == "api-subway:ecosystem" and item["value"] == "npm"
                for item in component["properties"]
            )
        )
        self.assertGreater(len(graph[esbuild["bom-ref"]]), 0)


class SchemaRuntimeTest(unittest.TestCase):
    def test_virtual_backend_runtime_requires_unique_primary_keys(self) -> None:
        schema = SCRIPTS.parent / "schemas/virtual-backend-v1.schema.json"
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "store.json"
            document.write_text(
                json.dumps({
                    "resources": {
                        "orders": {
                            "primaryKey": "id",
                            "records": [{"id": "same"}, {"id": "same"}],
                        }
                    },
                }),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "duplicate"):
                validate_schemas.validate_virtual_backend_runtime(schema, document)


if __name__ == "__main__":
    unittest.main()
