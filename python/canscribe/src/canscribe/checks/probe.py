import subprocess
import sys

from .types import CheckResult


def probe_gpu_runtime() -> list[CheckResult]:
    """Run a minimal torch GPU workload and check for runtime issues.

    Requires torch to be installed. Gracefully handles torch not being
    available.
    """
    try:
        import torch
    except ImportError:
        return [CheckResult("INFO", "Runtime probe", "torch not available — skipping")]

    if not torch.cuda.is_available():
        return [CheckResult("INFO", "Runtime probe", "no GPU available — skipping")]

    probe_code = """
import torch

x = torch.ones((1,), device="cuda")
y = x + 1
torch.cuda.synchronize()
result = float(y.cpu()[0])

maps_lines = []
with open("/proc/self/maps") as f:
    for line in f:
        if "libdrm_amdgpu" in line:
            maps_lines.append(line.strip())

print(f"GPU_RESULT={result}")
print(f"GPU_NAME={torch.cuda.get_device_name(0)}")
print(f"GPU_MEMORY={torch.cuda.get_device_properties(0).total_memory / 1024**3:.1f} GiB")
for ml in maps_lines:
    print(f"MAPS:{ml}")
    break
"""
    probe = subprocess.run(
        [sys.executable, "-c", probe_code],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )

    stderr_ids_count = sum("amdgpu.ids" in line for line in probe.stderr.splitlines())
    gpu_result = ""
    gpu_name = ""
    gpu_memory = ""
    loaded_path = ""
    for line in probe.stdout.splitlines():
        if line.startswith("GPU_RESULT="):
            gpu_result = line.removeprefix("GPU_RESULT=")
        elif line.startswith("GPU_NAME="):
            gpu_name = line.removeprefix("GPU_NAME=")
        elif line.startswith("GPU_MEMORY="):
            gpu_memory = line.removeprefix("GPU_MEMORY=")
        elif line.startswith("MAPS:"):
            fields = line.removeprefix("MAPS:").split()
            loaded_path = fields[-1] if fields else ""

    if probe.returncode != 0:
        detail = probe.stderr.strip().splitlines()
        last = detail[-1] if detail else f"exit {probe.returncode}"
        return [CheckResult("FAIL", "Runtime probe", f"GPU kernel failed: {last}")]

    results = [
        CheckResult(
            "OK",
            "GPU kernel",
            f"{gpu_name}, {gpu_memory} — result={gpu_result}",
        )
    ]
    if stderr_ids_count:
        results.append(
            CheckResult(
                "WARN",
                "Runtime stderr",
                f"amdgpu.ids error appears ({stderr_ids_count} line"
                f"{'s' if stderr_ids_count > 1 else ''})",
                explanation=(
                    "The bundled libdrm_amdgpu.so prints this message to stderr "
                    "each time it fails to open the hardcoded path. "
                    "The error is harmless — GPU operations work correctly — "
                    "but it is noisy and confusing."
                ),
                fix=(
                    "Create a symlink so the bundled lib finds the file: "
                    "sudo mkdir -p /opt/amdgpu/share/libdrm && "
                    "sudo ln -s <nix-store-path>/share/libdrm/amdgpu.ids "
                    "/opt/amdgpu/share/libdrm/amdgpu.ids"
                ),
            )
        )
    else:
        results.append(CheckResult("OK", "Runtime stderr", "no amdgpu.ids errors"))

    if loaded_path:
        if sys.prefix != sys.base_prefix and "site-packages" in loaded_path:
            results.append(
                CheckResult(
                    "WARN",
                    "Runtime libdrm loading",
                    f"{loaded_path} loaded (bundled, not nixpkgs)",
                    explanation=(
                        "libamdhip64.so has RPATH $ORIGIN, which resolves to torch/lib/ "
                        "at load time. The dynamic linker checks RPATH before "
                        "LD_LIBRARY_PATH, so the bundled (venv) libdrm_amdgpu.so is "
                        "loaded instead of the nixpkgs one — even though nixpkgs has "
                        "the correct path."
                    ),
                    fix="See the 'Bundled libdrm' WARN above for a suggested fix.",
                )
            )
        else:
            results.append(CheckResult("OK", "Runtime libdrm loading", loaded_path))
    return results
