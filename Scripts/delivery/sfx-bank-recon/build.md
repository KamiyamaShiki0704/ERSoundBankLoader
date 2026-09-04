---
id: FEAT-ConditionalSfxBank
type: build
version: 2.0
updated: 2026-09-05
based_on:
  - spec.md@2.0
  - design.md@2.0
---

# Configurable SFX Build Verification

Version 2.0 adds generic repeated weapon and SpEffect mappings, a bounded configurable readiness
timeout, public generic documentation and a backward-compatible experimental-enable alias.

## Identity and Scope

- Project base revision: `c57126855f44515af329bd10c7164f1fbae809f6` plus uncommitted SFX changes.
- Knowledge-base revision: `0333a2ab2077c824e257148439db5ecdf4765f90`.
- Upstream FSRS remains pinned to `902851865d05069eda7dcfd6385c8881eadf29bf`.
- Compiled input hashes are recorded in the private experiment's `manifest.json`.
- Changed runtime files: `src/lib.rs` and new `src/sfx.rs`. No Cargo version/dependency changes.
- Public INI/README now document generic default-off configuration. Active DLL/INI and the v0.3.0
  publication remain unchanged during this phase.

## Automatic Results

| Check | Result / evidence |
| --- | --- |
| `cargo check --locked` | PASS |
| `cargo fmt --check` | PASS |
| `cargo test --locked -- --nocapture` with ER_GAME_EXE set | PASS: 20 tests, including SFX weapon/SpEffect union and timeout config |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| `cargo build --release --locked --target-dir target/sfx-experiment` | PASS |
| Existing ER compatibility PowerShell probe | PASS against file version 2.7.0.0 |
| SFX executable fingerprint test | PASS; modified acquire code rejected |
| Offline Python recon unit tests | PASS: 7 tests |
| Package-script PowerShell syntax | PASS |
| ZIP content hashes | PASS: all five entries match staged files |
| Independent read-only code review | No actionable findings; lifecycle/INI/vector offsets checked |

The executable SHA-256 was
`D1A84083C6C7C7902162FF098F7D86812839AA6B3575959398857E539C488134`.
Native calls were NOT executed during these checks.

## Spec Coverage

- SPEC-SFX-001: config/mapping tests and native code fingerprints pass. Cold-load appearance P1
  remains manual, NOT RUN.
- SPEC-SFX-002: repeated demand is idempotent; two category mappings for one Bank deduplicate.
  Ten fake-backend cycles balance request/release. Live repeated switching P2 is NOT RUN.
- SPEC-SFX-003: simulated external ownership survives release; pending reference is released
  once. Native shared character P3 and live-effect completion P4 are NOT RUN.
- SPEC-SFX-004: missing acquisition is latched until demand ends; pending timeout does not
  reacquire, cancel or claim readiness; late completion works; lost identity stops calls.
  Missing-file native failure P5 is NOT RUN.
- SPEC-SFX-005: existing sound tests pass, SFX defaults off, active deployment hashes unchanged.
  Live sound/SFX coexistence remains manual.
- SPEC-SFX-006: repeated weapon/SpEffect parser and trigger-union tests pass; identical Bank IDs
  deduplicate through the shared demand set.
- SPEC-SFX-007: SFX-only and combined startup branches compile; invalid settings fail closed;
  public INI/README use generic placeholders and keep SFX off by default.
- SPEC-SFX-008: 1000-120000 ms parser bounds and configurable Driver timeout tests pass.

No live automatic FrameRecorderProbe is supplied or executed: this is a native ER DLL project,
not a UE project, and no suitable runtime Recorder infrastructure is present. The scope is
offline unit/static verification plus user-owned runtime acceptance, not automatic game PASS.

## Delivery

Latest private local package: `dist/ERSoundBankLoader-configurable-sfx-test4.zip`.
Staged files: `dist/ERSoundBankLoader-configurable-sfx-test4/`.

- DLL: 318464 bytes; SHA-256
  `166B67075E4B7C64C1B9109CD727270705873F33EF47EFE2457169B9CB197ABC`.
- ZIP SHA-256: `88D4070D4870AA43C67FFC6884C14469F041986B8FBDCE37C13FD0DC33E9FE9F`.
- Test INI snapshots the user's active sound mappings and adds SFX 86:2120 plus logging=true.
- Contains no game assets. Private config must not be published.
- Packaging leaves the active pair untouched; no installation, Git commit/push or release update.

## Remaining Risks

FileStepUpdate_Begin is source-backed precedent, not proven synchronization with all CSSfx
workers. Native concurrency, address reuse, world transitions, GPU readiness, native owner
behavior and rendering remain unverified. Unverifiable ownership disables the experiment for
the session and can retain a bounded reference until restart. These guards do not make native
calls crash-proof. Final PASS/FAIL belongs to the user after manual testing.
