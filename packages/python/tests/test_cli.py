from __future__ import annotations

import os
from pathlib import Path
import signal
import subprocess
import sys
import unittest


class WrapperSmokeTest(unittest.TestCase):
    def test_forwards_to_native_binary(self) -> None:
        binary = os.environ.get("API_SUBWAY_TEST_BINARY")
        self.assertIsNotNone(binary, "API_SUBWAY_TEST_BINARY is required")
        package_root = Path(__file__).resolve().parents[1]
        environment = {
            **os.environ,
            "API_SUBWAY_BINARY": str(binary),
            "PYTHONPATH": str(package_root),
        }
        result = subprocess.run(
            [sys.executable, "-m", "api_subway", "--help"],
            cwd=package_root,
            env=environment,
            check=False,
        )
        self.assertEqual(result.returncode, 0)

    @unittest.skipIf(sys.platform == "win32", "POSIX signal semantics")
    def test_propagates_native_binary_signal(self) -> None:
        environment = os.environ.copy()
        environment["API_SUBWAY_BINARY"] = sys.executable
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "api_subway",
                "-c",
                "import os, signal; os.kill(os.getpid(), signal.SIGTERM)",
            ],
            cwd=Path(__file__).resolve().parents[1],
            env=environment,
            check=False,
            timeout=30,
        )
        self.assertEqual(result.returncode, -signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
