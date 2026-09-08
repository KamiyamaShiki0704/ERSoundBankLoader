# Elden Ring 1.17.1 Compatibility

Project: ERSoundBankLoader, based on release commit
`eeac7d3fd22af8df2e890925805f48a1a8eebbe5`; compatibility build 0.4.1.

## Executable Identity

The inspected local executable has internal file version `2.7.1.0`, timestamp `0x6a96b418`,
SizeOfImage `0x5e0da00` and SHA-256
`1a3547101327f65d0c76da2f9190ac0aa66871ea42bae2aecc61e11a8b597891`.
Game patch 1.17.1 and internal file version 2.7.1.0 are different version labels.

## Required Addresses

All addresses are RVAs relative to the executable base, not process addresses.

| Function | 2.7.0.0 | 2.7.1.0 | Handling |
| --- | --- | --- | --- |
| Task registration | 0xEB3DE0 | 0xEB3E50 | Existing runtime signature resolves correctly |
| Wwise LoadBankMemoryCopy | 0x223BDC0 | 0x223BE30 | Existing export lookup resolves correctly |
| Wwise UnloadBank | 0x2242C70 | 0x2242CE0 | Existing export lookup resolves correctly |
| CSSfx acquire path | 0xD90CF0 | 0xD90D60 | New version profile |
| CSSfx release cap | 0xD91020 | 0xD91090 | New version profile |
| CSSfx collector | 0xD931C0 | 0xD93230 | New version fingerprint |
| CSFile release implementation | 0x2654170 | 0x26541E0 | Same bytes, relocated |
| CSFile reference decrement | 0x2654210 | 0x2654280 | Same bytes, relocated |
| FFXBND factory | 0x1F1D10 | 0x1F1D10 | Same entry, changed internal relative calls |
| Bundle use increment | 0x20F2B0 | 0x20F2B0 | Same entry and bytes |
| Bundle use decrement | 0x20F320 | 0x20F320 | Same entry and bytes |
| FFXBND processing | 0x20F340 | 0x20F340 | Same entry, changed internal calls |

A blanket +0x70 adjustment would break the unchanged functions.

RTTI confirms unchanged vtable RVAs: CSSfxImp `0x2BC31A0`, CSFileImp `0x29D4800`,
FfxbndFileCap `0x29DAAE0`. Native construction and acquire/release instructions confirm unchanged
singleton slots CSSfx `0x3D87A28` and CSFile `0x3D5F158`. The CSFile vtable release entry now points
to `0x2656110`; it remains an indirect game call and is not hard-coded by the loader.

## Instruction and Dependency Audit

The comparison script checks retained 2.7.0 disassembly against the new binary for acquire (127
instructions), release (22), factory (73), collector (164) and processing (807). All 1193
instructions preserve positions within their respective functions. Changed instructions are
relocated direct calls, equivalent local branches or RIP operands addressing unchanged data.
Other instructions, including reference-count, list and child-vector field accesses, match.
Evidence is stored locally in ignored `dist/sfx-recon/271-comparison.json`; no game disassembly
or executable is shipped in the package.

The loader's FSRS call paths use reflected singleton discovery, `SoloParamRepository::get`
through direct parameter structures, `SpecialEffect::entries`, and player-set `characters`.
These paths do not invoke the eldenring `rva::get()` version dispatch. The project already uses
its own signature-resolved task-registration call instead of the upstream static task API.
The pinned FSRS commit remains `902851865d05069eda7dcfd6385c8881eadf29bf`.

This audit does not establish that every player/object field is unchanged. It covers the native
SFX functions/fields above and the project's API dependencies. Player layouts and effect behavior
still need in-game confirmation on the new patch.

## Fix and Verification

The original executable test failed with unsupported SFX identity. Separate profiles now select
the correct native entry points and validate eight full function ranges before use. The original
2.7.0 identity, addresses and fingerprints are retained. Unknown identities remain rejected.

- 21 Rust tests pass with the new executable supplied through ER_GAME_EXE, including actual
  task/Wwise discovery, SFX fingerprints, all-eight-range mutation rejection and profile selection.
- Cargo check, formatting and strict Clippy pass.
- New binary comparison passes against retained 2.7.0 instruction evidence.
- Full original 2.7.0 executable was not available for a new end-to-end old-binary test in this
  run; profile selection is tested and original byte hashes remain unchanged.

Reproduce the static checks from the project root:

```powershell
$env:ER_GAME_EXE = '<path-to-eldenring.exe>'
cargo test --locked -- --nocapture
python -B Scripts/delivery/sfx-bank-recon/script/compare_271.py --exe $env:ER_GAME_EXE --evidence dist/sfx-recon --output dist/sfx-recon/271-comparison.json
```

The Python comparison additionally needs the retained local evidence and Python pefile/capstone.
No test here launches the game or calls native SFX routines. Use the existing configured weapon
and SpEffect cases to verify playback and switching in-game; startup logs should identify SFX
profile `2.7.1.0`. Keep the user's existing INI when testing the compatibility DLL.
