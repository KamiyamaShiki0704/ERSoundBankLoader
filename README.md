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
- Preserves banks already present in Wwise and banks subsequently needed by native characters.
- Conditionally requests native `SfxBnd_c####.ffxbnd` SFX Banks by weapon `sp_atkcategory` or
  active SpEffect, with demand deduplicated across all players and both trigger types.
- Releases only its own SFX Bank reference after the last matching condition ends; native resource
  management decides when the bundle can be physically collected.
- Logs AKRESULT and the real Wwise bank id to `ersoundbank_loader.log`.

This version does not expose an HKS/Lua `act(...)` command. Runtime state is controlled by DLL-side condition detection.

## Compatibility Check

The compatibility probe verifies the task-registration signature, named Wwise exports, FD4
singleton-reflection signatures, and required singleton names in an Elden Ring executable:

```powershell
.\Scripts\delivery\er-2.7-compat\script\Test-ERSoundBankCompatibility.ps1 -GameExe "<path-to-eldenring.exe>"
```

The Sound Bank implementation is statically verified against Elden Ring `2.7.1.0`, `2.7.0.0` and the retained
`2.6.2.0` reference executable. Runtime symbol resolution supports these versions in the same DLL.
SFX compatibility is checked separately; see the [1.17.1 compatibility report](Scripts/delivery/sfx-bank-recon/compatibility-1.17.1.md).

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

[conditional_sfx_bank_mappings]
sfx_enabled=false
sfx_ready_timeout_ms=10000
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

The loader initially tracks ownership from `LoadBankMemoryCopy`. `AK_BankAlreadyLoaded` means the
Bank already existed; the DLL borrows it and skips `UnloadBank` when its conditions end.
`AK_Success` initially gives the DLL permission to unload its own load.

While holding that permission, each condition poll also checks native characters' primary and
additional Sound Bank requirements, including the default character Bank. If a native character
needs the same Bank, the DLL relinquishes unload permission for that tracked load. This also
covers a Boss appearing after a weapon loaded its Bank. Switching weapons then leaves cleanup
to the game, even if the character subsequently disappears. Banks used only by configured weapon
or SpEffect triggers still unload normally. Incomplete scene or NpcParam inspection defers owned
unloads until inspection recovers.

This is conservative protection based on native character demand, not a Wwise ownership query.
The log records `conditional protected bank`, `ownership=native_character` and incomplete-inspection
deferrals. Native users entirely between polls and non-character users remain runtime limitations.
See the [shared Sound Bank fix](Scripts/delivery/shared-sound-bank/build.md) for verification and
the manual Boss test.

The old comma-separated keys are still accepted for compatibility.

SFX Bank loading is configured independently of Sound Bank loading. It does not use
`conditional_enabled`, `memory_copy_enabled`, `unload_when_inactive`, or `bank_file_dir`.
Enable it and add one mapping per line:

```ini
[conditional_sfx_bank_mappings]
sfx_enabled=true
sfx_ready_timeout_ms=10000
sfx_condition_bank_mapping=<sp_atkcategory>:<SFX-Resource-Bank-ID>
sfx_sp_effect_bank_mapping=<SpEffect-ID>:<SFX-Resource-Bank-ID>
```

Both trigger types scan every player in `WorldChrMan.player_chr_set`. If multiple players or
conditions request the same SFX Bank, the DLL acquires one native reference. It releases that
reference only after no configured trigger requests the Bank. Loading a Bank does not spawn or
play an effect; it makes the bundle resources available for actions that request them.

The SFX Resource Bank ID maps to the game's virtual path
`sfxbnd:/SfxBnd_c####.ffxbnd`. The corresponding `sfxbnd_c####.ffxbnd.dcx` must be available through
the active game or mod VFS. Do not point `bank_file_dir` at SFX files; that option is only for loose
Wwise `.bnk` files.

`sfx_ready_timeout_ms` accepts 1000 through 120000 milliseconds. A timeout is diagnostic: the DLL
does not repeatedly request or forcibly cancel the bundle while the condition remains active.
The log distinguishes condition demand, native request, resource-ready observation, release, and
fail-closed shutdown. Resource-ready does not prove visual output; actions and effects still need
in-game validation.

Native SFX support has separate guarded profiles for internal executable versions `2.7.0.0` and
`2.7.1.0` (game patch 1.17.1). The latter moves native acquire/release functions while leaving
some resource helpers unchanged. Both profiles validate code fingerprints before use. If the
executable identity, native code, manager identity, or retained resource ownership does not match,
the SFX subsystem disables itself without disabling the existing Sound Bank path. Native task
concurrency and all map-transition cases are not fully proven, so test new mappings offline first.

FD4 filecap requests alone were not enough to make sounds play, so that obsolete diagnostic path is
no longer called. The loader resolves the exported `LoadBankMemoryCopy` and `UnloadBank` functions by
name, while the internal task-registration function is found with a unique runtime signature. This
avoids tying the DLL to the function RVAs of one Elden Ring executable version.
