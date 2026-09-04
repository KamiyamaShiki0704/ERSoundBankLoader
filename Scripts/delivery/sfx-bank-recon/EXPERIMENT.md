# ERSoundBankLoader: SFX Experiment 1

Private offline test, based on v0.3.0. Not a public release or validated multiplayer feature.
Supported SFX executable: inspected Elden Ring 2.7.0.0 only. Code/manager mismatches disable SFX.
The original sound path retains its existing behavior and upstream dependency pin.

## Test Configuration

This package contains a snapshot of your existing INI with its sound mappings preserved, plus:

```ini
[sfx_experiment]
sfx_enabled=true
sfx_ready_timeout_ms=10000
sfx_condition_bank_mapping=86:2120
log_enabled=true
```

These keys are independent of conditional_enabled and unload_when_inactive, which govern sound.
The SFX experiment always releases its own demand when the selected weapon condition ends.
SFX shares condition_hand and poll_every_frames with the existing player scanner.
Additional weapon mappings can use repeated sfx_condition_bank_mapping lines. SpEffect-triggered
Banks use repeated `sfx_sp_effect_bank_mapping=<SpEffect-ID>:<SFX-Resource-Bank-ID>` lines.
Multiple players and both trigger types share one demand per Bank. No public default mapping is changed.

The native virtual path is `sfxbnd:/SfxBnd_c2120.ffxbnd`; the physical bundle must be available
through the active game/mod mounts. The package does not contain the game's SFX assets.

## Installation and Rollback

Close the game before selecting the experimental DLL/INI in your normal DLL-loading setup.
Use exactly one ERSoundBankLoader instance; loading the release and experiment simultaneously
would create two independent sound/resource owners. The sidecar must retain the name
ersoundbank_loader.ini beside er_sound_bank_loader.dll.

Packaging does not install anything or alter your live DLL/INI. Retain the original pair for
rollback. Disable sfx_enabled and restart to stop SFX loading; restore the
original DLL/INI pair for a full rollback. INI is read on startup, not hot-reloaded.
This package embeds your private sound settings and must not be uploaded as a public release.

## Manual Test

1. Start a fresh offline session in an area without the source character or an SFX carrier.
2. Confirm the chosen action's effect was unavailable in the baseline. Switch to special weapon
   category 86, wait for loading, then test the actions you selected.
3. Switch away and back ten times. Check effect appearance and existing weapon sound behavior.
4. Separately test a native character sharing Bank 2120: switching your weapon away must not
   remove that character's effects.
5. When applicable, switch away during a persistent effect and verify it finishes normally.
6. A controlled missing-Bank fixture is a separate failure test, not a change to the shipped
   86:2120 mapping. A timeout must not cause repeated outstanding requests.

Immediate first-frame effects are excluded: resource loading is asynchronous. No automatic
playback is added by loading the Bank. Visual/gameplay PASS or FAIL is determined by the user.

## Log Interpretation

Inspect ersoundbank_loader.log beside the loaded DLL. The log may contain previous sessions;
locate the latest `sfx enabled for ER 2.7.0.0` startup marker.

- `sfx demand banks=[2120]`: the weapon condition was detected.
- `bank=2120 requested cap=...`: one native reference was acquired, not proof of visible effects.
- `resources_ready`: cap state is Ready and a nonempty FXR vector was observed. Texture/model
  vector counts are included. This is not a GPU-readiness or visual-success guarantee.
- `timeout`: 10 seconds elapsed without resource-ready observation. The single request remains;
  it is not repeatedly acquired or forcibly cancelled. Switching away releases its extra use.
- `released own reference`: DLL demand ended. The game may keep the Bank for other owners or
  active effects; immediate physical unloading is neither forced nor promised.
- `sfx disabled`: configuration or executable guard rejected the experiment.
- `STOPPED`: identity/ownership became unverifiable. Further SFX calls are disabled for this
  session; some references may remain until restart. Do not treat this as a clean unload.

Attach the latest log and describe which action failed, whether the source character was nearby,
and whether failure occurred on initial load, switching, death, travel or returning to the menu.

## Known Risks and Verification Boundary

The file-stage choice has a resource-loading precedent, but concurrency with every native SFX
task/worker is not proven. Native calls can still crash; identity checks are not a game-wide lock.
No DLL detach-time cleanup, arbitrary FXR overrides or devpatch Bank support is attempted.
Seamless Co-op remains outside the first test's acceptance scope. SpEffect mappings now use the
same all-player scanner but still require separate in-game acceptance.

Automated checks cover state transitions, configuration, code fingerprints and existing sound
logic. They do not execute native SFX loading or render effects. Consult manifest.json for the
exact binary/source identities, and the project build.md for automatic verification results.
