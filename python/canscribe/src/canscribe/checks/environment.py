import os
from pathlib import Path

from .types import CheckResult, Status


def check_ld_library_path() -> list[CheckResult]:
    """Validate all entries in LD_LIBRARY_PATH exist."""
    ld_path = os.environ.get("LD_LIBRARY_PATH", "")
    if not ld_path:
        return [CheckResult("INFO", "LD_LIBRARY_PATH", "not set")]

    entries = [entry for entry in ld_path.split(":") if entry]
    missing = [entry for entry in entries if not Path(entry).is_dir()]
    ok_count = len(entries) - len(missing)
    detail = (
        f"{ok_count} valid, {len(missing)} missing"
        if missing
        else f"{ok_count} entries, all valid"
    )
    status: Status = "OK" if not missing else "WARN"
    results = [CheckResult(status, "LD_LIBRARY_PATH", detail)]
    results.extend(CheckResult("WARN", "  missing entry", entry) for entry in missing)
    return results


def check_key_env_vars() -> list[CheckResult]:
    """Check presence of important environment variables."""
    results: list[CheckResult] = []
    checks = (
        ("HF_TOKEN", "HuggingFace token for model access"),
        ("ROCR_VISIBLE_DEVICES", "ROCm visible devices"),
        ("HIP_VISIBLE_DEVICES", "HIP visible devices"),
        ("CUDA_VISIBLE_DEVICES", "CUDA visible devices"),
    )

    for var, purpose in checks:
        if os.environ.get(var, ""):
            results.append(CheckResult("OK", var, purpose))
        else:
            results.append(CheckResult("INFO", var, f"not set ({purpose})"))
    return results


def check_python_path() -> list[CheckResult]:
    """Check PYTHONPATH entries for validity."""
    pp = os.environ.get("PYTHONPATH", "")
    if not pp:
        return [CheckResult("INFO", "PYTHONPATH", "not set")]

    entries = [entry for entry in pp.split(":") if entry]
    missing = [entry for entry in entries if not Path(entry).exists()]
    if missing:
        results = [
            CheckResult(
                "WARN",
                "PYTHONPATH",
                f"{len(entries)} entries, {len(missing)} missing",
            )
        ]
        results.extend(
            CheckResult("WARN", "  missing entry", entry) for entry in missing
        )
        return results

    return [CheckResult("OK", "PYTHONPATH", f"{len(entries)} entries, all valid")]
