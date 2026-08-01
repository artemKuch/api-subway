from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import tempfile


def _binary_path() -> Path:
    override = os.environ.get("API_SUBWAY_BINARY")
    if override:
        return Path(override)
    executable = "api-subway.exe" if sys.platform == "win32" else "api-subway"
    bundled = Path(__file__).resolve().parent / "bin" / executable
    if not bundled.is_file():
        raise RuntimeError(
            "The native api-subway binary is missing from this wheel. "
            "Reinstall a wheel that matches the current platform."
        )
    return bundled


@contextmanager
def _executable_binary() -> Iterator[Path]:
    binary = _binary_path()
    if sys.platform == "win32" or os.access(binary, os.X_OK):
        yield binary
        return

    try:
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
        yield binary
        return
    except OSError:
        pass

    with tempfile.TemporaryDirectory(prefix="api-subway-") as directory:
        temporary = Path(directory) / binary.name
        shutil.copyfile(binary, temporary)
        temporary.chmod(temporary.stat().st_mode | stat.S_IXUSR)
        yield temporary


def main() -> int:
    try:
        with _executable_binary() as binary:
            return _run_binary(binary)
    except (OSError, RuntimeError) as error:
        print(f"api-subway: {error}", file=sys.stderr)
        return 2


def _run_binary(binary: Path) -> int:
    process = subprocess.Popen([str(binary), *sys.argv[1:]])
    forwarded = tuple(
        candidate
        for name in ("SIGHUP", "SIGINT", "SIGTERM")
        if isinstance((candidate := getattr(signal, name, None)), signal.Signals)
    )
    previous_handlers: dict[signal.Signals, signal.Handlers] = {}

    def forward(signum: int, _frame: object) -> None:
        if process.poll() is None:
            try:
                process.send_signal(signum)
            except ProcessLookupError:
                pass

    try:
        for forwarded_signal in forwarded:
            previous_handlers[forwarded_signal] = signal.getsignal(forwarded_signal)
            signal.signal(forwarded_signal, forward)
        return_code = process.wait()
    finally:
        for forwarded_signal, handler in previous_handlers.items():
            signal.signal(forwarded_signal, handler)

    if return_code < 0 and os.name != "nt":
        child_signal = -return_code
        if child_signal in forwarded:
            signal.signal(child_signal, signal.SIG_DFL)
        os.kill(os.getpid(), child_signal)
        return 128 + child_signal
    return return_code
