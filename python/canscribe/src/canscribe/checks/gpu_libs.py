import os
import re
import sys
from pathlib import Path

from .types import CheckResult


def _find_project_root() -> Path:
    """Walk up from sys.prefix / cwd to find the repo root with pyproject.toml."""
    for base in (Path(sys.prefix).parent, Path.cwd()):
        for parent in (base, *base.parents):
            if (parent / "pyproject.toml").exists():
                return parent
    return Path.cwd()


_BUNDLED_PATTERNS = (
    "torch/lib/libdrm_amdgpu.so",
    "triton/backends/amd/lib/libdrm_amdgpu.so",
)
_PRINTABLE_BYTES = re.compile(rb"[\x20-\x7e]{4,}")
_VERSION = re.compile(rb"(?<![0-9.])[0-9]+\.[0-9]+\.[0-9]+(?![0-9.])")


def _bundled_libdrm_paths(root: Path) -> list[Path]:
    site_packages = root / ".venv/lib/python3.13/site-packages"
    return [
        site_packages / pattern
        for pattern in _BUNDLED_PATTERNS
        if (site_packages / pattern).exists()
    ]


def scan_bundled_libdrm(root: Path) -> list[tuple[Path, str]]:
    """Find bundled libdrm_amdgpu.so files and hardcoded paths inside them."""
    results: list[tuple[Path, str]] = []
    seen: set[tuple[Path, str]] = set()
    for full in _bundled_libdrm_paths(root):
        try:
            data = full.read_bytes()
        except OSError:
            continue
        for raw in _PRINTABLE_BYTES.findall(data):
            if not raw.startswith(b"/") or b"amdgpu.ids" not in raw:
                continue
            item = (full, raw.decode("ascii"))
            if item not in seen:
                seen.add(item)
                results.append(item)
    return results


def check_bundled_libdrm_paths() -> list[CheckResult]:
    """Check if bundled libdrm_amdgpu.so has stale hardcoded /opt/amdgpu/ paths."""
    root = _find_project_root()
    bundles = scan_bundled_libdrm(root)
    if not bundles:
        return [
            CheckResult("INFO", "Bundled libdrm", "no bundled libdrm_amdgpu.so found")
        ]

    nix_ids_path: str | None = None
    for entry in os.environ.get("LD_LIBRARY_PATH", "").split(":"):
        if not entry:
            continue
        candidate = Path(entry) / "libdrm_amdgpu.so"
        if candidate.exists():
            candidate_ids = candidate.parent.parent / "share/libdrm/amdgpu.ids"
            if candidate_ids.exists():
                nix_ids_path = str(candidate_ids)
                break

    results: list[CheckResult] = []
    for lib_path, hardcoded_path in bundles:
        rel_lib = os.path.relpath(lib_path, root)
        if Path(hardcoded_path).exists():
            results.append(
                CheckResult(
                    "OK",
                    "Bundled libdrm",
                    f"{rel_lib} → {hardcoded_path} (found)",
                )
            )
            continue

        explanation = (
            "libamdhip64.so and libdrm_amdgpu.so both use RPATH $ORIGIN, "
            "which resolves to torch/lib/ at load time. "
            "The dynamic linker checks RPATH before LD_LIBRARY_PATH, "
            "so the bundled (venv) lib is always loaded instead of the "
            "nixpkgs version — even though the nixpkgs lib is on "
            "LD_LIBRARY_PATH with the correct amdgpu.ids path."
        )
        fix = None
        if nix_ids_path:
            fix = (
                "sudo mkdir -p /opt/amdgpu/share/libdrm && "
                f"sudo ln -s {nix_ids_path} /opt/amdgpu/share/libdrm/amdgpu.ids"
            )
        results.append(
            CheckResult(
                "WARN",
                "Bundled libdrm",
                f"{rel_lib} has stale path {hardcoded_path} → file not found",
                explanation=explanation,
                fix=fix,
            )
        )
    return results


def check_bundled_libdrm_version_mismatch() -> list[CheckResult]:
    """Compare nixpkgs-provided libdrm version with bundled version."""
    root = _find_project_root()
    bundled_versions = [
        f"{path.name} ({version})"
        for path in _bundled_libdrm_paths(root)
        if (version := _extract_so_version(path))
    ]

    nix_libraries: list[tuple[Path, str | None]] = []
    for entry in os.environ.get("LD_LIBRARY_PATH", "").split(":"):
        if not entry:
            continue
        candidate = Path(entry) / "libdrm_amdgpu.so"
        if candidate.exists():
            nix_libraries.append((candidate, _extract_so_version(candidate)))

    if not bundled_versions and not any(version for _, version in nix_libraries):
        return []

    results: list[CheckResult] = []
    if any(version for _, version in nix_libraries):
        for path, version in nix_libraries:
            rel = os.path.relpath(path, root)
            results.append(
                CheckResult(
                    "INFO",
                    "libdrm (nixpkgs)",
                    f"{rel} ({version})" if version else rel,
                )
            )

    if bundled_versions:
        results.append(
            CheckResult(
                "INFO",
                "libdrm (bundled)",
                "; ".join(bundled_versions),
                explanation=(
                    "These bundled libs are loaded at runtime due to RPATH $ORIGIN, "
                    "bypassing the nixpkgs version on LD_LIBRARY_PATH."
                ),
            )
        )
    return results


def _extract_so_version(path: Path) -> str | None:
    """Extract a version string directly from shared-object bytes."""
    try:
        data = path.read_bytes()
    except OSError:
        return None
    for raw in _PRINTABLE_BYTES.findall(data):
        if match := _VERSION.search(raw):
            return match.group().decode("ascii")
    return None
