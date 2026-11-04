"""Run the bundled binary, replacing this process.

`os.execv` matters: wayfinder serves MCP over stdin and stdout, so the binary must
inherit this process's descriptors rather than talk through a pipe a parent owns.
"""

import os
import sys
from pathlib import Path

BINARY_NAME = "wayfinder"


def binary_path() -> Path:
    """Path to the binary shipped inside this wheel."""
    return Path(__file__).resolve().parent / "bin" / BINARY_NAME


def main() -> None:
    """Replace the current process with wayfinder."""
    binary = binary_path()
    if not binary.is_file():
        raise SystemExit(
            f"wayfinder: the binary is missing from this installation ({binary}).\n"
            "Reinstall the package, or install the released binary directly:\n"
            "  curl -fsSL https://raw.githubusercontent.com/zaterka/wayfinder/main/scripts/install.sh | bash"
        )
    try:
        os.execv(str(binary), [str(binary), *sys.argv[1:]])
    except OSError as error:  # pragma: no cover - exec only fails on a broken install
        raise SystemExit(f"wayfinder: cannot run {binary}: {error}") from error
