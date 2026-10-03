# Branch consolidation, 2026-10-03

Baseline: `0c986996a732fab139fb1721adb045eb56fe3756` on the default
`trunk` branch. Remote default was
`bcb91bb0dd4e4de2e9d56f8b95a0677d7d1276f2`.

The reconciliation preserves newer implementations rather than restoring older
branch trees. Patch-equivalence alone is insufficient: merge-tree conflicts and
the affected source were inspected for the remaining semantic changes.

| Branch | Inspected tip | Disposition |
| --- | --- | --- |
| `feat/paperclip-reader` | `7805454b5fe2943f99b10ffd8df3d5a6d4b1bc61` | Merged by `9d62e28a6c025c78f3f7849b9c7a30a6221ee78e`: run-bound reader, sealed publication evidence, bounded/revocable transport and historical build ownership. Combined schema version 7 retains session knowledge and replication. Current packaging and environment inputs are preserved. |
| `integrate/harbor-db-storage-20261002` | `c16057631a96be5a7a1cd508ad67447625b640d7` | Qualified Harbor DB pin and formatting already present. Carry forward the stronger exact zero-inference syntax assertions in `nix/typedb-vm-test.nix`, preserving the newer mandatory federation gate. |
| `sync/simit-fixes-20260930` | `d1ec1fa71f5246cf20ca3c6f4b83ad3d3633cbd7` | Managed workflow and packaging fixes already present. Current workflow extends its gates with exact Nix outputs and federation tests; preserve the newer configuration. |
| `typedb-bootstrap` | `6f93080ce257f7ba5a7354a77f09a4551921be68` | Console-only bootstrap, credential snapshots, interruption recovery, service readiness and lifecycle VM cases already present. Differences are formatter-neutral shell/Nix layout, option inheritance and newer historical cutover documentation. |
| `pilot-hardening` | `1abadc68f82b0ee08e5615d7d1999c6fe6f601d7` | Patch-equivalent deterministic-only/symlink hardening already present as `5053874`; later extraction and zero-inference tests extend it. |
| `review/session-research-20260930` | `543aceabf00d4ef34a58b14b850ec21ee7193bf9` | Research/source validation patches already present; the later pinned Jev-only implementation supersedes its OpenCode inference/auth-seeding machinery. Preserve the current cited finite-choice research path. |
| `integration/scratch-canix-rollout` | `a62a8463388695bae8a88ba2e685702db875f04e` | Ancestor of baseline; scratch and hosted nextest fixes retained. |
| `land-bootstrap` | `2496a3671dcde79527902ac9cdcac6f54c5d4123` | Ancestor of baseline. |
| `session-verifier` | `4d45dc00fecdf0c7e9a5156a76fc25aeab79cd78` | Ancestor of baseline; later verifier contracts retained. |
| `origin/session-intelligence` | `7ee9412d42d0a45c329bf30a29f15c1c2a8c64c4` | Ancestor of baseline; later evidence/custody/uncertainty changes retained. |
| `origin/typedb-bootstrap` | `6bd7df2718d908570dde32b7040a5dc59ffff655` | Included in the audited newer local bootstrap tip above. |
| `qualify/federation-readonly-20261003` | `47b40e01b7d367fdc1095d4ba3d0feb911403160` | Baseline contains the first three qualification commits. The additional exact CLI diagnostic assertion and active follow-up gate edits require their owner's finalized commit before this stream can be declared consolidated. |

Reader merge verification: workspace native tests, strict workspace/all-target
Clippy, scoped pinned treefmt and Nix parsing passed. Ordinary tests do not
provision TypeDB; native live-gate eligibility returns and ignored integration
tests are not live-server evidence. Exact-source package/VM/consumer qualification
and final fleet readiness remain separate gates.

Branch references and existing worktrees are retained. An absorbed branch is
accounted for here without manufacturing an empty merge commit. Concurrent
federation-gate edits are preserved for their owner to finalize.
