# ERSoundBankLoader

Elden Ring DLL for preloading character Sound Banks without spawning hidden 0-HP enemies.

## Current Scope

- Loads as a native `cdylib`.
- Waits for `CSTask` to exist.
- Schedules retry attempts on `CSTaskGroupIndex::SoundStep`.
- Logs FD4 `wwise:/####.bnk` filecap requests for diagnostics.
- Reads the local player's active weapon `sp_atkcategory`.
- Loads the mapped `cs_c####.bnk` file with `AK::SoundEngine::LoadBankMemoryCopy`.
- Unloads previously loaded dynamic banks with `AK::SoundEngine::UnloadBank` when the condition no longer matches.
- Logs AKRESULT and the real Wwise bank id to `ersoundbank_loader.log`.

This version does not expose an HKS/Lua `act(...)` command. Runtime state is controlled by DLL-side condition detection.

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

For dynamic load/unload mode, set `conditional_enabled=true` and add one mapping per line:

```ini
[conditional_bank_mappings]
condition_bank_mapping=<sp_atkcategory>:<chr-sound-bank-id>
```

`bank_file_dir=auto` searches for `sd` and `mod/sd` near the DLL, near the game exe, and under nearby project folders. You can still add explicit folders with repeated `bank_file_dir=...` lines.

The old comma-separated keys are still accepted for compatibility.

`SOUND_BANK_LOADER_SKIP_CHR_IDS` in `EnemySummon.lua` only controls whether the old hidden carrier summon is skipped; it does not load or unload banks.

FD4 filecap requests alone were not enough to make sounds play. The confirmed working step is `LoadBankMemoryCopy`.
