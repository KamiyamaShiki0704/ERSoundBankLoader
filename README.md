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

Place `ersoundbank_loader.ini` beside the DLL. Defaults currently target the banks used by `EnemySummon.lua`:

```ini
[general]
enabled=true
log_enabled=true

[bank_files]
memory_copy_enabled=true
bank_file_dir=auto

[runtime]
conditional_enabled=true
condition_hand=auto
poll_every_frames=6
unload_when_inactive=true

[conditional_bank_mappings]
condition_bank_mapping=73:7800
condition_bank_mapping=105:8019
condition_bank_mapping=122:4205
condition_bank_mapping=132:9495
condition_bank_mapping=133:9495
condition_bank_mapping=643:9496
```

Set `log_enabled=false` to disable `ersoundbank_loader.log` output.

When `conditional_enabled=true`, `condition_bank_mapping` entries control which bank is loaded for each detected `sp_atkcategory`. `sound_bank_id` entries are only a fallback startup preload list for `conditional_enabled=false`.

`bank_file_dir=auto` searches for `sd` and `mod/sd` near the DLL, near the game exe, and under nearby project folders. You can still add explicit folders with repeated `bank_file_dir=...` lines.

The old comma-separated keys are still accepted for compatibility.

`SOUND_BANK_LOADER_SKIP_CHR_IDS` in `EnemySummon.lua` only controls whether the old hidden carrier summon is skipped; it does not load or unload banks.

FD4 filecap requests alone were not enough to make sounds play. The confirmed working step is `LoadBankMemoryCopy`.
