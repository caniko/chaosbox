import os
import re
import subprocess
from pathlib import Path

from .types import CheckResult


def _get_runpath_rpath(so_path: Path) -> tuple[str | None, str | None]:
    """Extract RUNPATH and RPATH from an ELF shared object."""
    try:
        result = subprocess.run(
            ["readelf", "-d", str(so_path)],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except FileNotFoundError:
        return None, None

    runpath = None
    rpath = None
    for line in result.stdout.splitlines():
        if match := re.match(
            r"\s*0x[0-9a-f]+\s+\(RUNPATH\)\s+Library runpath:\s*\[(.+)\]",
            line,
        ):
            runpath = match.group(1)
        if match := re.match(
            r"\s*0x[0-9a-f]+\s+\(RPATH\)\s+Library rpath:\s*\[(.+)\]",
            line,
        ):
            rpath = match.group(1)
    return runpath, rpath


def check_library_resolution(so_name: str = "libdrm_amdgpu.so") -> list[CheckResult]:
    """Trace which copy of a given .so would be loaded based on RPATH/RUNPATH."""
    results: list[CheckResult] = []
    ld_path = os.environ.get("LD_LIBRARY_PATH", "")
    ld_preload = os.environ.get("LD_PRELOAD", "")

    if so_name in ld_preload:
        results.append(
            CheckResult(
                "INFO", f"{so_name} resolution", f"LD_PRELOAD contains {so_name}"
            )
        )

    ld_copies = [
        Path(entry) / so_name
        for entry in ld_path.split(":")
        if entry and (Path(entry) / so_name).exists()
    ]
    results.append(
        CheckResult(
            "INFO",
            f"{so_name} on LD_LIBRARY_PATH",
            "; ".join(map(str, ld_copies)) if ld_copies else "not found",
        )
    )

    root = Path.cwd()
    parents_to_check = (
        root / ".venv/lib/python3.13/site-packages/torch/lib/libamdhip64.so",
        root / ".venv/lib/python3.13/site-packages/torch/lib/libdrm_amdgpu.so",
        root
        / ".venv/lib/python3.13/site-packages/triton/backends/amd/lib/libdrm_amdgpu.so",
    )
    for path in parents_to_check:
        if not path.exists():
            continue
        runpath, rpath = _get_runpath_rpath(path)
        tag = "RPATH" if rpath else ("RUNPATH" if runpath else "no RPATH/RUNPATH")
        tag_detail = rpath or runpath
        detail = (
            f"{tag}: {tag_detail.replace('$ORIGIN', str(path.parent))}"
            if tag_detail
            else tag
        )
        results.append(CheckResult("INFO", f"  {path.name}", detail))
    return results
