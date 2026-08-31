# ERSoundBankLoader

Elden Ring DLL for preloading character Sound Banks without spawning hidden 0-HP enemies.

## Current Scope

- Loads as a native `cdylib`.
- Waits for `CSTask` to exist.
- Schedules retry attempts on `CSTaskGroupIndex::SoundStep`.
- Resolves the task scheduler and Wwise functions at runtime instead of using version-specific RVAs.
- Reads the active weapon `sp_atkcategory` and configured active SpEffects of every player in
  `WorldChrMan.player_chr_set`.
- Loads the mapped `cs_c####.bnk` file with `AK::SoundEngine::LoadBankMemoryCopy`.
- Keeps a mapped bank loaded while any local or remote player matches either trigger type.
- Unloads previously loaded dynamic banks with `AK::SoundEngine::UnloadBank` when no player matches.
- Never unloads a bank that was already present in Wwise before the DLL requested it.
- Logs AKRESULT and the real Wwise bank id to `ersoundbank_loader.log`.

This version does not expose an HKS/Lua `act(...)` command. Runtime state is controlled by DLL-side condition detection.

## Compatibility Check

The compatibility probe verifies the task-registration signature, named Wwise exports, FD4
singleton-reflection signatures, and required singleton names in an Elden Ring executable:

```powershell
.\Scripts\delivery\er-2.7-compat\script\Test-ERSoundBankCompatibility.ps1 -GameExe "<path-to-eldenring.exe>"
```

The current implementation is statically verified against Elden Ring `2.7.0.0` and the retained
`2.6.2.0` reference executable. Runtime symbol resolution allows both versions to use the same DLL.

## Building

Build the Release DLL with:

```powershell
cargo build --release
```

No sibling or local `fromsoftware-rs` checkout is required. Cargo fetches `eldenring` and
`fromsoftware-shared` from `vswarte/eldenring-rs`, with both dependencies pinned to commit
`902851865d05069eda7dcfd6385c8881eadf29bf` for reproducible source selection.

## Config

Place `ersoundbank_loader.ini` beside the DLL. A minimal config looks like this:

```ini
[general]
enabled=true
log_enabled=true

[runtime]
start_delay_ms=5000
conditional_enabled=false
condition_hand=auto
poll_every_frames=6
unload_when_inactive=true

[bank_files]
memory_copy_enabled=true
bank_file_dir=auto
```

Set `log_enabled=false` to disable `ersoundbank_loader.log` output.

With the minimal config, the DLL loads but does not preload any custom Sound Banks until you add entries.

For startup preload mode, keep `conditional_enabled=false` and add one bank per line:

```ini
[fallback_startup_preload]
sound_bank_id=<chr-sound-bank-id>
```

For dynamic load/unload mode, set `conditional_enabled=true`. A Bank can be triggered by an active
weapon `sp_atkcategory`, an active SpEffect, or both:

```ini
[conditional_bank_mappings]
condition_bank_mapping=<sp_atkcategory>:<chr-sound-bank-id>
sp_effect_bank_mapping=<SpEffect-ID>:<chr-sound-bank-id>
```

Conditional mode scans all player characters on every poll, including remote players in Seamless
Co-op. Joins, leaves, weapon swaps, hand-style changes, and configured SpEffect changes are detected
dynamically. Only SpEffect IDs present in `sp_effect_bank_mapping` entries are retained in condition
snapshots and logs. When multiple trigger entries request the same Bank, it is loaded once and stays
loaded until every matching weapon and SpEffect trigger has become inactive. Sound Bank state is
local to each game process, so every player who needs to hear these sounds must install the DLL and
the same `.bnk` files.

The loader tracks bank ownership from the result of `LoadBankMemoryCopy`. `AK_Success` means the DLL
performed the load and may later unload it. `AK_BankAlreadyLoaded` means the game or another system
loaded it first; the DLL borrows that bank and skips `UnloadBank` when the weapon condition ends.
This prevents mapped weapons from unloading a Sound Bank still needed by a map Boss.

The old comma-separated keys are still accepted for compatibility.

FD4 filecap requests alone were not enough to make sounds play, so that obsolete diagnostic path is
no longer called. The loader resolves the exported `LoadBankMemoryCopy` and `UnloadBank` functions by
name, while the internal task-registration function is found with a unique runtime signature. This
avoids tying the DLL to the function RVAs of one Elden Ring executable version.
