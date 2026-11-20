# launcher/core — Manual and Native Verification Plan

These checks are for the user to run. The agent did not run any of them and
claims no PASS. Each item gives acceptance criteria, not a reproduction recipe.
Record each result as `PASS` / `FAIL` / `PENDING` in the result table at the end,
with the evidence directory. Change a finding's status in `docs/TASKS.md` or
`docs/launcher-core-remediation.md` only when every criterion of its item is PASS.

## Common rules

- Work in a fresh clone or worktree at the commit under test. Record
  `git rev-parse HEAD`, `git status --porcelain=v1`, `rustc -vV`, `cargo -V`,
  the OS version, the filesystem type, and `sha256sum Cargo.lock` before and after.
  Any change to `Cargo.lock` means FAIL.
- Use a new empty directory for every run. Never reuse evidence directories.
- Never point the launcher at real instances, the real launcher config, the real
  SQLite database, or real credentials. Set a root override to a fresh temp directory.
- Do not log `CURSEFORGE_API_KEY` or any other credential.
- Unset `DATABASE_URL`, `CURSEFORGE_API_KEY` and `SONATA_TEST_CONTROLLED_CHILD`
  unless an item says otherwise.

Isolated environment (bash; use the PowerShell equivalent on Windows):

```bash
repo=<path to checkout>
run=$(mktemp -d)
mkdir -p "$run"/{tmp,config,data,cache,cargo-cwd,evidence}
export TMPDIR="$run/tmp" XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" \
       XDG_CACHE_HOME="$run/cache" SQLX_OFFLINE=true
unset DATABASE_URL CURSEFORGE_API_KEY SONATA_TEST_CONTROLLED_CHILD
cd "$run/cargo-cwd"
```

The Python platform runner (`docs/verify-platform.py`) was removed in `382ce7d`.
Until it is restored, run the cargo pipeline directly (item 1).

## 1. Native Windows / macOS pipeline

- Host: native Windows 10 1607+ (NTFS) and macOS (APFS). Toolchain: stable Rust
  with the native target (MSVC or GNU on Windows; record which).
- Commands (each output tee'd into `$run/evidence/<step>.log`):
  `cargo fmt --manifest-path "$repo/Cargo.toml" -- --check`,
  `cargo check --manifest-path "$repo/Cargo.toml" --locked --all-targets`,
  `cargo test --manifest-path "$repo/Cargo.toml" --locked`,
  `cargo build --manifest-path "$repo/Cargo.toml" --locked`,
  `cargo clippy --manifest-path "$repo/Cargo.toml" --locked --all-targets`.
- Expected: all steps exit 0; test totals have 0 failed (Linux reference at
  `a8b3c97`: 665 lib + 2 CLI + 1 + 5 integration + 18 doctests, 1 ignored).
  Platform-gated tests (`cfg(windows)`, `cfg(unix)`) must appear in the output.
  Zero selected tests for a gate is not a PASS.
- Also confirm that these tests ran and passed: the Windows junction/reparse fixtures
  (`archive_extraction_declines_a_linked_output_directory`,
  `copy_replace_refuses_a_redirected_destination_directory`), the macOS held
  directory tests (`directory_publication_declines_a_changed_destination`),
  and cleanup/quarantine replay (`scratch_cleanup_preserves_a_replacement_directory`,
  `abandoned_scratch_cleanup_preserves_a_replacement`).
- Status rule: findings marked `PLATFORM VALIDATION PENDING` move only with PASS on
  both native OSes, or PASS on one plus an explicit note that the other is out of scope.

## 2. B02 on native Windows

- Host: native Windows, NTFS. Filter: `cargo test --locked --lib legacy`. Then
  run the full suite once.
- Expected: unique renamed/disabled legacy hardlinks are detached using physical
  identity (volume serial + 128-bit file ID). Ambiguous, missing, replaced or corrupt
  CAS cases are preserved and reported, not changed. All 18 B02 regressions PASS.
- Evidence: test log, plus `report.json` if the runner has been restored.
- Status: B02 stays `PARTIAL` either way. A PASS only removes "native Windows pending".

## 3. F04 / F07 / F14 / F15 / F22 native

- NTFS and APFS. Run the filtered tests `identity`, `legacy`, `path_key`,
  `ownership_`, and `portable`, then the full suite.
- Manual acceptance on an isolated root:
  - Case and normalization aliases of one file are not treated as two owners.
  - Coincident configured roots take each lock once.
  - After an injected SQL failure following a filesystem success, a restart
    finishes the replay or leaves the record unchanged, never half-applied.
  - A managed Java home registered on Windows round-trips as a portable relative
    path and launches a real `java.exe -version`.
  - A historical Windows SQLite fixture, built by you from a disposable install,
    upgrades without losing rows.
- Evidence: logs plus a copy of the disposable DB before and after (never a real DB).

## 4. F12 / F21 native

- Run the filtered tests `processor` and `process_pin` on Windows and macOS.
- Expected: processor output is truncated at its budget with one notice, and pipes
  do not deadlock. Known pre-spawn failures release every pin. Pins and scratch stay
  in place while a process's state is unknown (for example, the launcher is killed
  while a processor runs) and are collected only after the exit is confirmed.

## 5. F02 processor workspace (code-level part done in `6bc4b9a`)

- Use an isolated root. Install real Forge and NeoForge versions:
  - at least one modern Forge with client processors;
  - one NeoForge;
  - one profile without client processors.
- Acceptance criteria:
  - The install completes, and the game launches to the main menu.
  - During processor runs, the instance game directory is not created or changed
    by the processor. Processor `ROOT` and temporary files sit under the
    installer work directory of the generation.
  - Processor logs (`processor-<n>.log` in the work directory) show no access
    to inherited launcher environment variables.
  - Cancelling during a processor stops the install and keeps the previous
    installation. After restart no half-published generation is used.
  - A failed processor keeps the previous installation and records a typed error.
  - No processor descendant process survives a cancelled install. Record the
    result, which is expected to FAIL today because descendants are not
    contained.
- Not provided and must stay `BLOCKED` until a mechanism is decided and verified
  on every OS: OS-level filesystem and network confinement, descendant
  containment, and checking undeclared write locations against declared outputs.

## 6. Clean install → prepare → launch

- Isolated root, DB and profile. For vanilla, Fabric/Quilt, Forge and NeoForge,
  record the loader, version, Java runtime, OS and result.
- Expected: launch reaches the main menu. Closing the game normally ends the
  operation as completed.

## 7. Content recovery (GUI snapshot with patches 01–05)

- Expected:
  - The pending list shows unfinished journals.
  - Retry succeeds after you move the conflicting file aside.
  - Confirmed abandon removes only the selected journal and its leases, and every
    file stays byte-identical (compare SHA-256 before and after).
  - A normal GUI close with a live game waits for shutdown and does not kill the game.
  - `Stop games` stops it explicitly.

## 8. Live JVMs and descendants

- Expected: detecting Java on PATH works on every OS. Pins for game and installer
  JVMs and their descendants are held while they run and released after a
  confirmed exit. Retention GC never collects a runtime in use.

## 9. Live providers

- Modrinth: already PASS on Linux; repeat on each native OS.
- CurseForge: run only by you, with `CURSEFORGE_API_KEY` set in the shell and not
  written to any log. Expected: search, exact release and dependency resolution
  succeed. Keep `MissingCurseForgeKey` without a key.

## 10. Hosted CI

- You push the branch and dispatch `.github/workflows/core-platform.yml`
  yourself. The agent does not push. A cross-build does not count as a native PASS.

## 11. Power-loss

- Not promised. The contract covers only process-crash recovery. Any power-loss
  experiment is a separate step you agree to, outside the release gate.

## Result table

| Item | Host / OS / FS | Commit | Result | Evidence dir | Date |
|---|---|---|---|---|---|
| 1 Windows | | | PENDING | | |
| 1 macOS | | | PENDING | | |
| 2 | | | PENDING | | |
| 3 | | | PENDING | | |
| 4 | | | PENDING | | |
| 5 | | | PENDING | | |
| 6 | | | PENDING | | |
| 7 | | | PENDING | | |
| 8 | | | PENDING | | |
| 9 | | | PENDING | | |
| 10 | | | PENDING | | |
