---
id: FIX-SharedSoundBank-Build
type: build
version: 1.0
updated: 2026-10-18
source: ai-derived
---

# Shared Boss Sound Bank Fix

Input contracts: spec.md@1.0 and design.md@1.0, SPEC-SOUND-001..005.
Project base: 86bb27cc6f01cc4497d3192e1ca5646efd6418d6, plus the source hashes in the delivery
manifest. Relevant KB input is lib-fsmod-engineering-experience@1.1, updated 2026-10-15.
Version 0.4.2 modifies conditional Sound Bank ownership, not SFX loading or the pinned FSRS commit.

## Finding and Fix

Initial ownership was based solely on the first LoadBankMemoryCopy result. The weapon-first load
returned AK_Success; a later native duplicate returned AK_BankAlreadyLoaded, but the DLL retained
its old unload permission. Switching away called UnloadBank and removed the shared Bank.

The production inactive-unload regression failed before the fix with:
`switching the weapon silenced the native Boss bank`. It now passes with the same load order.
Native primary/additional character demand is checked each condition poll while a DLL-owned
Bank is tracked. Observing matching demand permanently relinquishes unload permission for that
tracked load; incomplete inspection defers remaining owned unloads. Game cleanup remains in control.

## Automatic Evidence

- 30 Rust tests PASS with the inspected ER 2.7.1.0 executable; no executable tests were skipped.
- SPEC-SOUND-001/P1/P2: late native claim and pre-existing native ownership unload tests PASS.
- SPEC-SOUND-002/P3: native claim recorded while a trigger stays active, then remembered after
  native disappearance and weapon switch PASS. Known claims survive a partial snapshot.
- SPEC-SOUND-003/P5: repeated incomplete inspection retains the Bank/tracking, then a complete
  empty native snapshot permits normal unload PASS.
- SPEC-SOUND-004/P4: inactive weapon-only Bank unloads; active weapon/SpEffect demand remains
  loaded without spuriously transferring ownership; existing union/deduplication tests PASS.
- SPEC-SOUND-005/P6: explicit, negative default, additional, zero and duplicate selection PASS.
- Formatting, strict Clippy and optimized MSVC release build PASS.
- Current EXE SHA-256: 1a3547101327f65d0c76da2f9190ac0aa66871ea42bae2aecc61e11a8b597891.
  Task/Wwise symbol resolution and existing SFX profile/mutation checks PASS.
- New DLL: 324096 bytes, SHA-256
  B83362E3CDD2E4AC35951EDB4699988B12E5FE892134253C6FD93DABC66323F6.

Reproduce from the project root:

```powershell
$env:ER_GAME_EXE = '<path-to-eldenring.exe>'
cargo test --locked -- --nocapture
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked --target-dir target/shared-boss-bank-fix
./Scripts/delivery/shared-sound-bank/script/Package-SharedSoundBankFix.ps1
```

The package script refuses to replace an existing delivery directory/ZIP. No test executes game
native Sound Bank functions: fixtures exercise the production ownership/unload path. Static field
and fallback evidence is bounded in design.md; the full additional-Bank activation path and
native map-load concurrency are not established. Native demand is not a Wwise ownership query.

## Manual Acceptance

Use the replacement DLL with the existing INI. For diagnostics, set log_enabled=true in that INI;
the generic packaged INI is not a replacement for the user's mappings. Restart the game so old
process-local ownership state is gone. Test weapon-first then matching Boss, and Boss-first then
weapon, switching away while the Boss performs audible actions. Also switch a weapon-only Bank
away in an area with no matching native character.

Expected diagnostics are version=0.4.2 at startup, conditional protected bank on observed native
demand, and unload skipped ownership=native_character when the final trigger ends. Unknown data
logs conditional unload deferred. Automated results do not constitute audible Boss acceptance.
Native users entirely between polls, non-character users, stale externally removed banks and
native loading/cleanup races remain runtime limits. No live installation or Git publication is
performed by this delivery. Local-only TASK_STATUS.md records package integrity and cleanup.
