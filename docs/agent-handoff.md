# launcher/core — Final Self-Contained Handoff for the Agent

This file is the **single source of the prompt**. It replaces reading `docs/`, the original audit from `/tmp`, and the journals: everything essential is copied or condensed below. If the agent receives only this file, it must work from it, must not ask for the rest of the documentation, and must not treat the absence of `/docs` as a reason to stop. In case of a discrepancy between historical wording and the current code, trust the current code; a status may change only after re-verifying the finding against the code.

---

## 0. Mission

Continue the remediation of launcher/core findings and close **everything that can be closed by code, tests, and contracts without human involvement**.

Priorities:

1. **Security and integrity (mandatory for full closure at the code level):** F02 (filesystem operation boundary and isolation of external installers), B01 (concurrent ancestor replacement), B02 (physical identity of legacy hardlinks), F04/F07/F14/F15 (identity, replay, paths, locks), F21 (processes with indeterminate outcome), Q12/API boundary (shutdown and absence of service bypasses).
2. **Everything else provable by automation:** regressions, contracts, owned workers, typed errors, debt partial items where the remainder is only historical/unknown data or a product decision.
3. **Prepare detailed manual/native checks:** the agent **does not execute** them and **does not declare PASS**; it writes a step-by-step plan for the user with expected results and evidence paths.

Forbidden:

- declaring `FIXED` without a deterministic regression check and a full run;
- turning `PARTIAL`/`BLOCKED` into `FIXED` by means of document wording;
- promising power-loss durability — the core contract is only **process-crash recovery**;
- guessing ownership/provenance of historical files and DB rows;
- switching the source GUI, changing its `Cargo.toml`/`Cargo.lock`, reading user databases/credentials;
- performing push/dispatch/commit to someone else's branch; all commits are local.

---

## 1. Repository, Starting Point, and Preservation of User Changes

- Absolute path: `/home/quartix/Projects/launcher-native/core-refactor`. **Not** `/home/quartix/Projects/launcher-native/core-refactor2` — that is an older copy at `db4de73` with untracked `docs/`.
- Branch: `refactor/all-audit-findings`.
- HEAD at the time of writing: `356c600336823dc5d0f848c42ac81c19375bbefb`.
- `git remote -v` is empty. History is local.
- Before edits, record: branch, HEAD, `git status --porcelain=v1`, staged diff, checksums of the three user files. Compare at the end.

The following must be preserved unchanged:

| File | State | SHA-256 at time of writing |
|---|---|---|
| `.cargo/config.toml` | modified (tracked) | `8f7cd9285cccbd108954979ec87e3d07597c32c5ab139b76d6ad9d0c46829e6f` |
| `Cargo.lock` | modified (tracked) | `45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3` |
| `not-a-database-url` | untracked | `aa318a137d9e804acbb47802d679028867a7b6df84b6618a33101cd6f6af5961` |

Also untracked: `docs/launcher-core-agent-handoff.md` (the previous short handoff) and this file. Do not delete or overwrite the short handoff.

If `Cargo.lock` changes as a result of the work — that is a failure, not a new baseline: revert the change, do not commit it, and record it in the report as a STOP reason. The platform runner also marks such a run `FAIL`.

---

## 2. Map of the Two Remediation Lines (important!)

The Git history of `core-refactor` is stitched together from **two independent audits**:

1. `… → bc1945d` — the `docs/TASKS.md` line: 73 A/B/D/Q/S/L items based on a static audit dated 17.11.2026 (`/home/quartix/Projects/launcher/core`, audit HEAD `4b445ba`, 178 Rust files, 23,179 lines). Line numbers and SHAs in that audit are historical, not current.
2. `d9131ac → … → 356c600` — the F01–F23 line from the `launcher-core-remediation.md` journal, based on a different audit (`/tmp/launcher-core-readonly.vnN6DeRh/launcher-core-audit.md`). 106 changed files, +7142/−1549.

The current code contains **both** lines. The short handoff describes only the second one. Therefore, "remaining tasks" = the union of:

- F02 plus the native gates from the F-line;
- the partial B02/B06/D08/S05/S06/P11-V02 items and the B01 window from the TASKS line;
- anything the agent finds still unclosed in code when re-verifying.

There is no need to "catch up" with the TASKS line again: it is already in the code. The point is not to lose its partial constraints and not to re-declare them closed.

---

## 3. Current Verified Baseline (HEAD `356c600`)

- Linux x86-64: `fmt --check`, `check --locked --offline --all-targets`, `test`, `build`, `clippy --all-targets` — PASS; **687 tests passed, 1 ignored**. Clippy emits warnings (not a blocker).
- Windows GNU from Linux: `check --all-targets`, `test --no-run`, `build` — compile/link PASS; **tests were not executed on Windows**.
- Windows MSVC: the C toolchain did not work; compile FAIL due to a dependency, not to core code.
- macOS: target/runner absent.
- Python verification runner: 8/8 PASS (these are runner contracts, not platform execution).
- Native Windows/macOS, hosted CI jobs, manual GUI/launch/install/content — **pending**.
- Real JVM detection `probes_java_on_path` PASS in a separate run: real JDK `27.0.1.0.3`. Live game/installer JVMs and their descendants — pending.
- Live providers: the Modrinth code path was executed against real responses (search/dispatch/normalize/exact_release PASS); CurseForge keyless → `MissingCurseForgeKey` before any network call, invalid key → `Err Http(403)`; the success path requires `CURSEFORGE_API_KEY`, which is not present on this machine.
- B01: the static path was correctly rejected 7/7 by the shipped `child()`; under concurrent ancestor replacement, 7/7 records ended up outside the root — the window is confirmed, the status was not changed.
- The source GUI is unmodified; patches 01–05 apply only to an isolated snapshot. P11-V01 fresh replay: 238 passed, 0 failed, 7 ignored. The explicit GUI check by the user is postponed, not cancelled.

---

## 4. F01–F23 Registry (from the short handoff, without /docs)

| ID | Verified issue | Current state |
|---|---|---|
| F01 | Publish the verified asset index at the game ID and preserve the previous index on failed reinstall | FIXED / VERIFIED |
| F02 | Maintain filesystem ownership during mutations and constrain external installer processes | **BLOCKED** |
| F03 | Bind staged content bytes to the recorded digest | FIXED / VERIFIED |
| F04 | Treat unknown physical identity as insufficient evidence for legacy detachment | FIXED / PLATFORM VALIDATION PENDING |
| F05 | Invalidate previews after SQL-only adoption | FIXED / VERIFIED |
| F06 | Bind SQLite ownership to a consistent root layout | FIXED / VERIFIED |
| F07 | Replay legacy detachment after filesystem success and SQL failure | FIXED / PLATFORM VALIDATION PENDING |
| F08 | Prevent multiple historical bindings from claiming the same placement | FIXED / VERIFIED |
| F09 | Reject ambiguous asset-index keys during ingestion | FIXED / VERIFIED |
| F10 | Share strict managed Java decoding across recovery and retention | FIXED / VERIFIED |
| F11 | Enforce transport policy per source while preserving explicitly configured mirrors | FIXED / VERIFIED, PARTIALLY CONFIRMED |
| F12 | Enforce archive, metadata, and processor output budgets | FIXED / PLATFORM VALIDATION PENDING |
| F13 | Apply shutdown admission to service mutators | FIXED / VERIFIED |
| F14 | Reconcile persisted path keys with filesystem identity and preserve ambiguous records | FIXED / PLATFORM VALIDATION PENDING, PARTIALLY CONFIRMED |
| F15 | Acquire coincident-root locks once | FIXED / PLATFORM VALIDATION PENDING |
| F16 | Reduce confirmed repeated scans and unnecessary digest work | FIXED / VERIFIED, PARTIALLY CONFIRMED |
| F17 | Preserve an unused preview after a transient Busy result | FIXED / VERIFIED |
| F18 | Preserve typed causes at verified API boundaries | FIXED / VERIFIED, PARTIALLY CONFIRMED |
| F19 | Preserve a manual Java choice during automatic selection | FIXED / VERIFIED |
| F20 | Keep startup recovery and its ownership guards alive after the caller drops | FIXED / VERIFIED |
| F21 | Finish known pre-spawn failures and retain resources when process state is unknown | FIXED / PLATFORM VALIDATION PENDING |
| F22 | Round-trip managed Java homes using portable relative paths | FIXED / PLATFORM VALIDATION PENDING |
| F23 | Retain layout ownership for the lifetime of exported services | FIXED / VERIFIED |

Total: 19 CONFIRMED, 4 PARTIALLY CONFIRMED. Verdict and state are different axes; for F11/F16/F18 the state refers to the confirmed part.

### 4.1. Essential F-line history (do not redo)

- `d9131ac` F01/F09: the asset index is published under the game ID with SHA-1/size verification, atomically, without overwriting; duplicate JSON keys are rejected before persistence. Regressions `asset_index_install_publication_matches_prepared_game_id_after_restart`, `asset_index_install_rejects_duplicate_keys_before_persistence`.
- `b33ceed` F22: portable `/` paths in `java/paths.rs`, operation records, home markers, SQL upgrade, retention. `nested_managed_home_uses_portable_paths_through_publication_and_recovery`, `managed_sql_encodes_nested_native_paths_as_portable_relative_paths`.
- `a8265a6` F06/F13/F15/F20/F23: ownership in `data/ownership.rs`, `data/db/mod.rs`, bootstrap; `ownership_*` regressions, coincident roots, caller-drop guards.
- `5d2cd98` F03: `modified_cas_bytes_are_never_published_under_the_registered_digest`; staged digest+size before rename.
- `43761e9` F04: `unknown_candidate_identity_defers_even_when_one_legacy_alias_is_identified`, `absent_cas_cannot_prove_ownership_of_a_recorded_path`.
- `8795203` F02 part: `initialization_refuses_an_assets_objects_directory_redirect`.
- `75d80ee` F05/F08/F17/F16: fingerprints of SQL facts, exact bindings, preview under mutex, reverse digest index.
- `eb1a2bb` F19: `java_auto_selection_preserves_a_manual_choice_made_before_update`.
- `2002dc6` F10: `duplicate_managed_record_keys_defer_recovery_and_collection`.
- `7accf62` F11/F12/F18: HTTPS policy, bounded reads, typed diagnostics, `redirects_follow_the_rules_for_https_and_configured_mirrors`, `url_policy_rejects_embedded_credentials_before_client_initialization`.
- `d6d3fd9` F12/F21: ZIP/log budgets, `missing_game_executable_releases_all_pre_spawn_pins`, `a_later_marker_failure_releases_earlier_pins_and_retains_unknown_markers`.
- `64287b4` F11: provider HTTPS, mirror exception, same-origin redirect; `5a0b0ed` F02/F07: held directories, replay of legacy migrations, migration 0016 intent.
- `2ed59d1` F02: held-directory archive/CAS writes; `aa87fa5` portability follow-up; `661d26b` downloads/placement; `25c0feb` managed Java publication/scratch; `a1f216c` evidence; `f3cf319` cleanup/metadata; `9350c7e` recovery/process records; `356c600` final F02 checkpoint.

### 4.2. F02 — what is already done and where the boundary remains

Done: operations from a held directory in `src/utils/files/directory.rs` and `directory/cleanup.rs`; publication and recovery of content from the root opened at bootstrap; CAS deletion via held directory; downloads into staging, Maven/assets placement, managed publication, ZIP/TAR extraction, scratch/natives/work/quarantine cleanup, metadata writes, Forge log, markers/locks/registry/operation records — via open parents. Marker formats, pre-existing destinations, and historic prefix recovery are preserved; ACL/xattrs are not promised.

Residual boundary (the reason for `BLOCKED`):

1. Identity is checked with one system call, while the file operation uses another. The hold guarantee relies on ownership of the working tree and on participant cooperation; an external writer with full access to the same tree remains outside the contract.
2. The Forge/NeoForge processor runs with launcher privileges. Isolation requires a supported mechanism on Linux/Windows/macOS and verification against standard installers.
3. Native Windows/macOS checks are unavailable; Windows GNU provides compile/link only.

Starting files: `src/utils/files/directory.rs`, `src/utils/files/directory/cleanup.rs`, `src/modloader/forge/install.rs`, `src/modloader/forge/processor.rs`, `src/instance/init/install.rs`, `src/instance/launch/resolved.rs`, `src/utils/files/mod.rs`, `src/utils/files/owned_directory.rs`, `src/instance/content/recovery.rs`.

F02 closure requirement: one processor contract; a private workspace as `ROOT`; read-only inputs; writes only to approved staging outputs; restriction of network, inherited environment, and child processes; the core verifies and publishes only approved results; unsupported installation — explicit refusal, with no silent degradation. Changing the supported loaders/versions is a product decision that must be escalated to the user, not made silently.

---

## 5. TASKS.md Registry and Its Open Tails

Total: 48 main + 8 persistence + 17 local = **73 items**. Most are closed. Not fully closed:

| ID | Status | What is already proven | What remains / why it cannot be closed automatically |
|---|---|---|---|
| B01 | Fixed on the normal path | missing/broken symlink and processor output path via `files::child`, regression for a missing target | Concurrent ancestor replacement: 7/7 outside root recorded; needs OS support for handles/no-follow and native validation. Security priority |
| B02 | Partial | Unique renamed/disabled legacy hardlinks are detached by physical identity: Unix device/inode, Windows volume serial + 128-bit file ID (`GetFileInformationByHandleEx`/`FILE_ID_INFO`, fail-closed `None`); owned Tasks worker holds activity + CAS admission + three lifetime locks after bootstrap drop until SQL completes; 18 Linux regressions; Windows GNU compile/link | Native Windows execution by the user (dual-boot). Ambiguous aliases/claims, missing/replaced CAS, unsafe/out-of-scope, corrupt CAS, and indeterminate ownership are deliberately deferred — guessing is forbidden. No durable migration journal in the FS→SQL window; power-loss is not promised |
| B06 | Partial | Installed seeds from consistent SQL object/provider bindings + full SHA-256 inventory snapshot; Required/Unknown for all active seeds in solver and catalog traversal; `NULL` → typed `MissingDependencyMetadata`, `[]` → known empty, corrupt JSON → Json; 25 regressions; 615 lib + 2 CLI + 2 int + 18 doctests | Historical metadata without dependency data and unknown/cross-provider identity are not guessed; live GUI/manual pending. Modrinth live path executed, CurseForge success path awaits a key |
| D08 | Partial | Scratch/admission, content GC, runtime generation/blob GC, asset CAS GC, managed staging/home/archive GC; startup collectors with complete owner roots, process/age gate, and replay; strict typed private asset index roots (duplicate/unknown/null rejected); 16 GC tests, 10 subprocess boundaries; 620 lib etc.; Windows GNU compile/link | Historical/unknown data; more aggressive placement GC requires ownership migration; real JVMs/descendants and Windows/macOS/power-loss are unverified |
| P11-V01 | Contract implemented | Core `9594de6`, refresh against `4f4f3b2`; GUI replay patches 01–05: 238 passed, 0 failed, 7 ignored; inventories/patch hashes/lock preservation | Manual native UI, live CurseForge, platform runtime are not PASS; source GUI not switched |
| P11-V02 | Partial | Native CI matrix configured; 8 Python regressions; Linux 572→620 lib; Windows GNU compile/link; 4 process regressions via controlled Rust child; Modrinth live code path PASS | Hosted CI jobs not run; native Windows/macOS, manual isolated launch/install/content/recovery/normal close, real JVMs/descendants, external concurrent FS/DB, power-loss — user/platform |
| S05 | Partial | Leases from preparing/download; content admission/roots/SQL locks; game/runtime roots and replay; asset workers; managed broad pin until production reads + independent game/probe/Forge child pins; 526 lib etc. | Real JVMs/descendants, live game/installer JVMs, external low-level consumer/API audit remain open |
| S06 | Partial | CAS paths from root/SHA-256; 0013 forbids `store_path`; root-aware upgrade writers for both legacy snapshots and linked_libs_at; managed Java SQL pair root-aware; 13 Java path tests; 474 lib etc. | Historical catalogue DB paths are not migrated automatically; external Java paths do not become managed; Windows/macOS/manual pending |

Other TASKS caveats: A07/P11-G01–G04/P10/Q items have Linux evidence but do not turn the whole P08/P11 into a blanket PASS; the Mods catalog in the GUI remains disabled; historical string UI adapters outside Java runtime/settings/content recovery are not declared migrated; abnormal renderer/process termination cannot be converted into a cooperative close.

### 5.1. TASKS-line commits relevant to the tails

`4f4f3b2` (Windows physical identity B02), `b20fe94` (ambiguous asset index roots D08), `9e17fff` (installed resolver B06), `77e5d34` (proven renamed legacy hardlinks B02), `7d9ac3a` (P11-V02 CI), `083d7cc`/`841a64b` (P11-V01), `b46e43d` (real JVM probe), `fe9d973`/`2ba9781` (live providers), `bc1945d` (B01 race observation).

---

## 6. Invariants and Contracts (inline; no /docs reading)

### 6.1. Publication and Recovery (process-crash, NOT power-loss)

`Completed` = filesystem operations + SQL commit both returned success; it is not a single FS+SQLite transaction. `atomic_write`/`publish_new` provide atomic visibility of an individual file and no-clobber behavior, but do not fsync directory entries. Assumptions: previous SQLite/root remains available, a local filesystem with ordinary rename/link semantics, and no external concurrent directory replacement. A final file modified by the user is preserved; automatic recovery stops on conflict; corrupt CAS is diagnosed, not overwritten.

Add-only install:

1. Under an activity lease + lifetime ownership locks, take shared CAS admission first, then write `preparing`; before that, CAS is not checked/imported.
2. Object lookup and lease are in one SQL transaction before reading bytes; registration/adoption and lease are atomic; FS import → SQL registration is protected by store-wide admission until completion/discard.
3. Copies/downloads go into a private `.sonata-content-*` outside `mods`; the full PID marker is published before useful files; hash/snapshot checks; cancellation before intent removes only `preparing` and leases.
4. One SQL transaction moves `preparing → publishing` plus the versioned file list and leases; after this, cancellation does not interrupt publication.
5. New files are no-clobber; a crash between files leaves the journal as the source for forward recovery.
6. One SQL transaction creates bindings/provenance, moves `publishing → completed`, and releases leases; the journal is then deleted. Startup replays only `publishing`: a matching file = external, a missing file = verified copy from CAS.

Recovery holds shared admission from reading the journal through SQL completion, with the clone in the copy/verify/publish worker. Exclusive capability by itself does not authorize collecting preparations/journals or published objects. Root transfer: the old preparation key is not used to clean another root.

B02 limitation: an SQL failure/collision after isolation leaves independent bytes and the original binding; a repeated startup is not required to automatically finish the rename based on guessed ownership; arbitrary concurrent writers/ancestor replacement are outside the contract.

### 6.2. Retention/GC

All four byte domains use the shared `utils::object_store` and **actual SHA-256 identity**; roles/placements/provenance are tracked separately; aliases/URLs/package metadata are hints, not proof of bytes. Copies/homes are independent; corrupt CAS is not overwritten. Startup collectors require complete owner roots, a process/age gate, and replayable intent/quarantine; unknown/historical/ephemeral owners, missing/replaced DB, pending/corrupt/future metadata fail closed; published homes/quarantine are not deleted under uncertainty. Physical namespaces are deliberately separate; Mojang SHA-1/virtual placements are not collected. No power-loss assumption is made.

### 6.3. Consumer/API boundary

`GlobalState`/`AppError` are roots; composition lives in `app`. The consumer works through `instances()`, `content()`, `java()`, `options()`, `versions()`, `modloaders()`. Raw Database/pool, mutable registries, low-level install/recovery/storage/process helpers, and shims are **not** public. `tests/public_api.rs` is the positive consumer; 18 `compile_fail` doctests in `src/lib.rs` reject bypasses. This is a Rust API boundary, not a sandbox against external FS/SQL writers.

- `JobId = OperationId` — session identity; content/managed UUID — durable domain identity, does not receive a session ID on recovery. `OperationSubject` is set before Start; the Display name is not protocol; legacy without a subject is uncorrelated.
- Bus subscribe **before** initial reconciliation; a pruned terminal = result unavailable, not success; lost output/duration are not reconstructed.
- Typed causes: `ManagedJavaErrorKind`, Unknown round-trip, retryability by typed transport/status/IO — not by code/text substring; a durable diagnostic is not reinterpreted; a runtime source is not serialized or fabricated after recovery.
- `catalog_report()`/`installations_report()` show partial warnings and record-local diagnostics; diagnostic-only changes participate in polling equality.
- `core.shutdown()` closes Tasks admission, sends cooperative cancel, waits for publishing/SQL/blocking cleanup without abort; games are not killed automatically; normal GUI close waits for shutdown, `Stop games` is an explicit action.
- CLI pure parse/help happens before dotenv/config/bootstrap: help/no args exit 0, syntax error exit 2 without root/DB/recovery side effects.

### 6.4. Installed-aware resolver (B06)

Installed seeds come only from consistent SQL object/provider bindings + a full inventory SHA-256 snapshot. The observed `.jar` is active, not the historical enabled flag; `.jar.disabled`, missing/modified/unstable/ambiguous rename metadata do not count. `NULL` dependencies → typed `MissingDependencyMetadata` and block the plan even for unrelated roots; `[]` → Known empty; corrupt/null/object JSON → `Json`. Fixed seeds remain selected during backtracking; `Unknown` relation and `Incompatible` in both directions; dedup by `(provider/project, exact_release)`; cycles, 4096 decisions, 128 request budget, 90s timeout are preserved. Multiple active bindings of one project with different release/deps → `ContentError::Invalid`, not a silent overwrite. Metadata is not written to SQL, and a similar/new release is not substituted.

### 6.5. Legacy hardlink migration (B02)

Input is an existing SQL hardlink binding. The recorded regular path has priority: current bytes are copied independently of CAS. Only when the path is missing — look for a unique CAS alias of the same `ContentKind` by physical identity; the digest is a lookup address, not proof of ownership. Unix device+inode, Windows volume serial + 128-bit file ID, fail-closed `None`; size/mtime/equal hashes do not replace identity. Scope: JAR/`JAR.disabled` in mods, ZIP in resourcepacks/shaderpacks; symlinks, symlinked directories, nested/unknown paths, and non-UTF-8 paths are not accepted. Exactly one alias and one possible historical binding are required; competing claims/occupied key → defer. Planned FileStamp before/after copy; persist replaces the proven legacy path; matching digest → placement `copy`, otherwise modified bytes = `external` and stale object/provider are cleared, corrupt CAS is not repaired. The SQL UPDATE re-checks previous ID/object/path/key/kind and the absence of a competing claim.

### 6.6. GUI patches 01–05 (snapshot only)

01 — closed service API + CLI root facade; positive consumer and 18 negative contracts. 02 — host credentials through `Config`, blank key excluded, cooperative normal-close without abort/timeout, repeat idempotent, Stop games explicit. 03 — `OperationSubject` before Start, generation token, subscribe before reconcile, fresh `operations()` on signal, explicit overflow, pruned = unknown result, one install slot. 04 — catalog/installations reports, diagnostics not erased, RecoveryAction by enum, unknown → Details/Inspect. 05 — recovery panel with the Mods catalog disabled, no automatic search, retry, confirmed abandon (files unchanged, only the selected journal/leases), stale/duplicate/busy gates, typed UiError. All source GUI files and dependency/lock remain original; the runner creates a copy in `/tmp/opencode`.

---

## 7. Verification Procedures (self-contained commands)

Cargo commands are run **outside** the repository/GUI config hierarchy, with the manifest passed explicitly. `SQLX_OFFLINE=true`, `--locked --offline`, separate temp directories, `OPENSSL_INCLUDE_DIR`/`OPENSSL_LIB_DIR` from `/usr/include/openssl-3` and `/usr/lib/openssl-3` if they exist. Do not use `DATABASE_URL`, `CURSEFORGE_API_KEY`, `SONATA_TEST_CONTROLLED_CHILD` from the environment.

```bash
repo=/home/quartix/Projects/launcher-native/core-refactor
run=$(mktemp -d /tmp/launcher-core-final.XXXXXXXX)
mkdir -p "$run"/{tmp,config,data,cache,cargo-cwd}

(
  unset DATABASE_URL CURSEFORGE_API_KEY SONATA_TEST_CONTROLLED_CHILD
  export TMPDIR="$run/tmp"
  export XDG_CONFIG_HOME="$run/config"
  export XDG_DATA_HOME="$run/data"
  export XDG_CACHE_HOME="$run/cache"
  export SQLX_OFFLINE=true
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}"
  if [[ -d /usr/include/openssl-3 && -d /usr/lib/openssl-3 ]]; then
    export OPENSSL_INCLUDE_DIR="${OPENSSL_INCLUDE_DIR:-/usr/include/openssl-3}"
    export OPENSSL_LIB_DIR="${OPENSSL_LIB_DIR:-/usr/lib/openssl-3}"
  fi

  cd "$run/cargo-cwd"
  cargo fmt --manifest-path "$repo/Cargo.toml" -- --check
  cargo check --manifest-path "$repo/Cargo.toml" --locked --offline --all-targets
  cargo test --manifest-path "$repo/Cargo.toml" --locked --offline
  cargo build --manifest-path "$repo/Cargo.toml" --locked --offline
  cargo clippy --manifest-path "$repo/Cargo.toml" --locked --offline --all-targets

  python3 -B -m unittest discover -s "$repo/docs" -p test_verify_platform.py
  python3 -B "$repo/docs/verify-platform.py" --offline --output "$run/linux"
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    python3 -B "$repo/docs/verify-platform.py" --offline \
    --target x86_64-pc-windows-gnu --output "$run/windows-gnu"
)
```

Focused regression (example): `cargo test --manifest-path "$repo/Cargo.toml" --locked --offline --lib <regression_filter>`. Confirm that the needed tests actually ran; zero selected tests is not PASS.

If `docs/verify-platform.py`/`docs/check.sh` are unavailable ("prompt-file only" mode): run the equivalent Cargo commands manually, honestly record `runner not executed`, and **do not** present runner evidence. For `--target` always use `cargo test --no-run` (compile-only), not execution. Evidence output must be fresh: the runner refuses to overwrite an existing directory.

A sandbox may block loopback HTTP fixtures — that is an environment limitation, not a code defect; record it as a separate reason and retry with 127.0.0.1 allowed. Timeout = FAIL, steps skipped after a failure = `not-run`, not PASS.

---

## 8. Work Rules

1. Before changing a verdict/scope, re-verify the finding against the current code. An audit is hypotheses and historical evidence, not proof of a current bug.
2. First reproduce the confirmed defect with a deterministic defensive regression through a production entry point, with isolated temp FS/SQLite and local HTTP fixtures. Avoid real Minecraft instances, launcher config, credentials, and user data.
3. Preserve behavior and historical data unless a defect is proven; migrations must be replayable; do not edit applied 0001–0008; add new constraints via following migrations with upgrade fixtures; keep unknown identity separate from proven mismatch; preserve ambiguous files/records; keep cleanup pins until proofs/process completion.
4. One logical commit = one task plus its tests/evidence, Conventional Commit (`fix(core):`, `test(core):`, `docs:`, etc.). Do not mix a functional fix, a mass rename, and a schema migration. Commits are local, with no push.
5. After each group: focused regressions + subsystem suite. At the boundary: fmt/all-target check/test/build/clippy/platform runner/Windows GNU cross. Clippy warnings are recorded, not hidden.
6. Update `docs/launcher-core-remediation.md` (the F-line journal) **and** `docs/TASKS.md` (the 73-item registry) for changed rows: independent verdict, root cause, fix, files, regression names, observed results, commit, remaining limits. If the `docs/` directory is absent or unavailable, create `agent-closure-continuation.md` at the repository root with the same columns and explicitly note that the main registries were not updated.
7. Do not delete unhooked code or shims "because they are old": inventory consumers first. Do not reintroduce low-level exports to bypass a service invariant.
8. Keep unmeasured performance concerns and optional removal of legacy helpers separate from release blockers.

---

## 9. Security Reporting (mandatory format)

This applies to F02, B01, B02, F04, F07, F14, F15, F21, the Q12/API boundary, and isolation of Forge/NeoForge processors.

**Code:** fix + deterministic regression tests with neutral names, isolated fixtures, and fail-closed behavior under uncertainty.

**Report/commit message/journal:** only

- the invariant (exactly what must be true),
- the affected component,
- the essence of the fix,
- the verification result (test names, passed/failed/ignored counts, platform scope),
- remaining limitations.

**Do not include** in reports, commit messages, and user instructions: step-by-step defect reproduction scenarios, specific bypass payloads/strings, race sequences, addresses and paths that recreate an out-of-bounds condition. Adversarial details stay inside tests and local logs with neutral names. Proof of closure is the tests and the run result, not a reproduction guide. This does not weaken the fix or change the contract: the guarantee remains the same.

Write manual checklists for the user as **acceptance criteria** (what observable must be true, which logs/evidence to keep), without reproducing the vulnerability.

---

## 10. What the Agent Must Prepare for the User (without executing)

Create a separate file `docs/manual-verification-plan.md` (if `docs/` is unavailable — `manual-verification-plan.md` at the repository root; a separate section in the report is also acceptable) with detailed but safe instructions. For each item: host/OS/toolchain/filesystem, exact commands, fresh temp directories, what NOT to touch (real instances/credentials/DB), expected observable result, log paths, where to record PASS/FAIL/PENDING, and criteria for changing status.

Minimum coverage list:

1. **Native Windows / macOS platform runner:** `python docs/verify-platform.py --output <new-dir>` on each OS; without `--offline` if dependencies are downloaded; record `core head`, `git status`, `rustc -vV`, `platform.platform()`, `lock_before/after`, native-core scope (not cross). Also check: Windows junction/reparse policy, macOS held-directory behavior, cleanup/quarantine replay.
2. **B02 native Windows:** a separate run with Windows physical identity (`FILE_ID_INFO`) and a real coalesced root; expected — unique renamed/disabled hardlinks detached, ambiguous/missing/corrupt preserved; evidence `report.json` + log.
3. **F04/F07/F14/F15/F22 native:** NTFS/APFS aliases, case/normalization, coincident roots, replay of FS success → SQL failure after restart, portable Windows managed Java home with a real executable, and a historical Windows SQLite fixture.
4. **F12/F21 native:** processor pipe/truncation behavior, known pre-spawn failures, process retention under indeterminate spawn/exit.
5. **F02 processor isolation:** clean install, output publication, rejection of operations outside the approved workspace, cancellation, descendant cleanup, failures preserve previous installations; with real Forge/NeoForge profiles, including profiles without a client processor. Do not disclose bypass details — only criteria.
6. **Clean install → prepare → launch** through isolated root/DB/profile; record loader, version, Java runtime, OS, result.
7. **Content recovery manually:** pending list, retry after preserving the conflicting file, confirmed abandon (only journal/leases, files untouched); native GUI normal close with a live game and `Stop games`.
8. **Live JVMs/descendants:** detection on PATH (already PASS), plus game/installer JVMs and descendants for retention pins; Windows/macOS JVMs.
9. **Live providers:** Modrinth code path already PASS; CurseForge success path with `CURSEFORGE_API_KEY` — user only, do not log the credential.
10. **Hosted CI:** push/dispatch of `.github/workflows/core-platform.yml` by the user; do not substitute a cross-build for native PASS.
11. **Power-loss:** not promised; if the user wants a dedicated experiment, it is a separate agreed step, not part of the release gate.

---

## 11. Completion Criteria

- Every changed finding has a current verdict + remediation status confirmed by source/test evidence. Allowed statuses:
  - `FIXED / VERIFIED` — deterministic regression + full available pipeline + no code-level limitations;
  - `FIXED / PLATFORM VALIDATION PENDING` — code is ready, but native OS/real process/key is unavailable;
  - `BLOCKED` — a technical/compatibility/product dependency prevents completion without a decision;
  - `PARTIAL` — the confirmed part is fixed, historical/unknown data deliberately preserved.
- Full available Linux pipeline PASS; Windows GNU reported separately from native Windows/macOS; manual plan handed to the user.
- The B01 concurrent window and F02 processor isolation are closed at the code level **or** explicitly `BLOCKED` with a precise reason and manual acceptance criteria; security reports contain no reproduction details.
- B02/B06/D08/S05/S06 are not re-declared: partial reasons are preserved in the registries.
- Final `git status`, checksums, and commit diffs show that user files were preserved.
- The final report states directly whether the core is ready for real user data and which operations remain possible only with disposable fixtures.

The current answer to the last question at the time of writing: **no, it is not ready** — F02 and native validation are open, plus the partial B02/B06/D08/S05/S06/P11-V02 items and manual checks. Status may change only on evidence collected according to the rules of this file.
