from .environment import check_key_env_vars, check_ld_library_path, check_python_path
from .gpu_libs import check_bundled_libdrm_paths, check_bundled_libdrm_version_mismatch
from .lib_path import check_library_resolution
from .probe import probe_gpu_runtime
from .types import CheckResult


def run_checks(probe: bool = False) -> list[CheckResult]:
    results: list[CheckResult] = []
    results.extend(check_ld_library_path())
    results.extend(check_python_path())
    results.extend(check_key_env_vars())
    results.extend(check_bundled_libdrm_paths())
    results.extend(check_bundled_libdrm_version_mismatch())
    results.extend(check_library_resolution())
    if probe:
        results.extend(probe_gpu_runtime())
    return results


def print_results(results: list[CheckResult], verbose: bool = False) -> None:
    for result in results:
        print(f"[{result.status:5s}] {result.name}: {result.detail}")
        if verbose and (result.explanation or result.fix):
            if result.explanation:
                for line in result.explanation.split("\n"):
                    print(f"       {line}")
            if result.fix:
                print(f"       fix: {result.fix}")
