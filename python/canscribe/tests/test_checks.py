import subprocess
import sys
from pathlib import Path

import pytest

from canscribe.checks.environment import (
    check_key_env_vars,
    check_ld_library_path,
    check_python_path,
)
from canscribe.checks.gpu_libs import scan_bundled_libdrm
from canscribe.checks.types import CheckResult


class TestScanBundledLibdrm:
    def test_no_files_found(self, tmp_path: Path) -> None:
        results = scan_bundled_libdrm(tmp_path)
        assert results == []

    def test_finds_hardcoded_path(self, tmp_path: Path) -> None:
        so_dir = tmp_path / ".venv/lib/python3.13/site-packages/torch/lib"
        so_dir.mkdir(parents=True)
        so_file = so_dir / "libdrm_amdgpu.so"
        so_file.write_bytes(b"/opt/amdgpu/share/libdrm/amdgpu.ids\x00garbage")

        results = scan_bundled_libdrm(tmp_path)
        assert len(results) == 1
        lib_path, ids_path = results[0]
        assert lib_path == so_file
        assert ids_path == "/opt/amdgpu/share/libdrm/amdgpu.ids"

    def test_finds_in_triton_too(self, tmp_path: Path) -> None:
        so_dir = tmp_path / ".venv/lib/python3.13/site-packages/triton/backends/amd/lib"
        so_dir.mkdir(parents=True)
        so_file = so_dir / "libdrm_amdgpu.so"
        so_file.write_bytes(b"/opt/amdgpu/share/libdrm/amdgpu.ids\x00")

        results = scan_bundled_libdrm(tmp_path)
        assert len(results) == 1

    def test_ignores_non_matching_files(self, tmp_path: Path) -> None:
        so_dir = tmp_path / ".venv/lib/python3.13/site-packages/torch/lib"
        so_dir.mkdir(parents=True)
        so_file = so_dir / "libdrm_amdgpu.so"
        so_file.write_bytes(b"some random bytes without the path")

        results = scan_bundled_libdrm(tmp_path)
        assert results == []


class TestCheckLdLibraryPath:
    def test_missing_entry(self, monkeypatch) -> None:
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {"LD_LIBRARY_PATH": "/nonexistent/path"},
        )
        results = check_ld_library_path()
        assert any(r.status == "WARN" for r in results)


class TestCheckKeyEnvVars:
    def test_reports_ok_for_set_var(self, monkeypatch) -> None:
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {"HF_TOKEN": "abc123"},
        )
        results = check_key_env_vars()
        hf = [r for r in results if r.name == "HF_TOKEN"]
        assert len(hf) == 1
        assert hf[0].status == "OK"

    def test_reports_info_for_unset_var(self, monkeypatch) -> None:
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {},
        )
        results = check_key_env_vars()
        hf = [r for r in results if r.name == "HF_TOKEN"]
        assert len(hf) == 1
        assert hf[0].status == "INFO"


class TestCheckPythonPath:
    def test_info_when_not_set(self, monkeypatch) -> None:
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {},
        )
        results = check_python_path()
        assert results[0].status == "INFO"

    def test_ok_when_all_valid(self, tmp_path: Path, monkeypatch) -> None:
        d = tmp_path / "p"
        d.mkdir()
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {"PYTHONPATH": str(d)},
        )
        results = check_python_path()
        assert results[0].status == "OK"

    def test_warn_on_missing_entry(self, monkeypatch) -> None:
        monkeypatch.setattr(
            "canscribe.checks.environment.os.environ",
            {"PYTHONPATH": "/does/not/exist"},
        )
        results = check_python_path()
        assert results[0].status == "WARN"


class TestCheckResultType:
    def test_named_tuple(self) -> None:
        r = CheckResult("OK", "test", "detail")
        assert isinstance(r, tuple)

    def test_fields(self) -> None:
        r = CheckResult("OK", "test", "detail")
        assert r.status == "OK"
        assert r.name == "test"
        assert r.detail == "detail"

    def test_optional_fields(self) -> None:
        r = CheckResult("WARN", "test", "detail", explanation="because", fix="do x")
        assert r.explanation == "because"
        assert r.fix == "do x"

    def test_optional_fields_default_none(self) -> None:
        r = CheckResult("OK", "test", "detail")
        assert r.explanation is None
        assert r.fix is None


class TestCliInvocation:
    @pytest.mark.parametrize(
        ("args", "timeout"),
        [
            (["canscribe", "doctor"], 30),
            (["canscribe", "doctor", "--verbose"], 30),
            (["canscribe", "doctor", "--probe"], 180),
        ],
    )
    def test_canscribe_doctor_variants_exit_zero(
        self, args: list[str], timeout: int
    ) -> None:
        result = subprocess.run(
            [sys.executable, "-m", "canscribe.main", *args[1:]],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        assert result.returncode == 0
