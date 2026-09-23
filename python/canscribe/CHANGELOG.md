# Changelog

## [Unreleased]

### Added

- **Diagnostics**: `canscribe doctor` supports `--verbose` and `--probe` flags,
  scans bundled ROCm libdrm for stale hardcoded paths, validates
  `LD_LIBRARY_PATH`, and traces RPATH/RUNPATH resolution.
- **ROCm stderr silencing**: `silence_stderr()` context manager suppresses the
  harmless `/opt/amdgpu/share/libdrm/amdgpu.ids: No such file or directory`
  noise from ROCm's bundled libdrm during the first GPU probe.
- **`--resume` / `-r` flag**: New flag on `transcribe` command. Parses existing
  transcript timestamps, skips already-processed segments, and saves
  incrementally so partial work survives crashes.
- **PyPI publish CI**: New Forgejo workflow triggered by version tags.
- **simit config**: Enables PyPI publishing.

### Changed

- Transcript output refactored around `append_segment` for incremental writing.
- `TranscriptionRequest` gains `resume: bool` field.
