# Native SFX Bank Reconnaissance

Static research, not a runtime implementation or an approved specification.
Current task status lives only in the project-root `TASK_STATUS.md`.

## Scope and Reproduction

Read-only analysis of an Elden Ring executable owned by the user. The tool does not open a game
process, inject code or change the executable. Generated reports stay under ignored `dist/`.
Do not commit game binaries or their disassembly reports.

Dependencies: Python 3, `pefile`, `capstone`.
Run from the project root, substituting the local executable path:

```powershell
python -B -m unittest discover -s Scripts/delivery/sfx-bank-recon/script -p 'test_*.py' -v
python -B Scripts/delivery/sfx-bank-recon/script/inspect_sfx.py strings --exe '<eldenring.exe>' --output dist/sfx-recon/strings.json
python -B Scripts/delivery/sfx-bank-recon/script/inspect_sfx.py field --exe '<eldenring.exe>' --offset 0x262 --output dist/sfx-recon/field-reads.json
python -B Scripts/delivery/sfx-bank-recon/script/inspect_sfx.py disasm --exe '<eldenring.exe>' --rva 0xd931c0 --output dist/sfx-recon/sfx-gc.json
```

Use `layout --rust '<pinned generated.rs>'` to compute the primitive repr(C) NPC layout.
`xrefs` finds candidate RIP-relative references; `callers` finds candidate direct calls.
Both accept repeated `--rva`. `disasm --size` can inspect a leaf function without unwind metadata.

Limitations: this is a narrow, heuristic analysis tool, not an exhaustive disassembler or Rust
parser. Only `.text` is scanned. Candidate scans omit indirect calls and functions without unwind
entries; non-overlapping byte searches can miss matches. Decoded instruction validation rejects
embedded opcode bytes but does not establish control-flow reachability. Explicit-size disassembly
can include padding or adjacent functions. `field` selects 16-bit operands only. `layout` assumes
the generated NPC struct uses primitive types and arrays with C alignment, not packed layout.
Negative search results do not prove absence. Function names below are analyst labels unless
explicitly backed by RTTI. RVAs are evidence for one executable, not shipping call addresses.

## Evidence Identity

- File version: `2.7.0.0`; preferred image base: `0x140000000`.
- SHA-256: `D1A84083C6C7C7902162FF098F7D86812839AA6B3575959398857E539C488134`.
- Pinned FSRS revision: `902851865d05069eda7dcfd6385c8881eadf29bf`.
- All binary JSON reports include this hash; `npc-field.json` instead identifies its Rust source.
- Evidence paths below are relative to project-root `dist/sfx-recon/`.

## Observed Native Path

| Stage | RVA / field | Evidence |
| --- | --- | --- |
| NPC parameter input | signed word at `+0x262`; read at `0x4cf0f1` | `npc-field.json`, `character-consumer.json` |
| Store selected Bank | `0x3f6220`: `ChrIns+0x28 -> ChrRes+0x170` | `bank-setter.json` |
| Character resource request | `0x4cb680` selects effective Bank at state `+0x1ec`, then requests it | `resource-state-init.json` |
| Bank path formatting | `0x3e6680`, `sfxbnd:/SfxBnd_c%04d.ffxbnd` | `path-xrefs.json`, `bundle-loaders.json` |
| Bank acquire / release | `0xd90f00` / `0xd91080` | `bundle-loaders.json`, `request-release.json` |
| Bundle request | `0xd90cf0` -> `0x1f1d10` | `request-release.json`, `filecap-factory.json` |
| Bundle processing | `0x20f340`, FFXBND vtable slot `+0x50` | `ffxbnd-processing.json` |
| Deferred collection | `0xd931c0` | `sfx-gc.json` |

RTTI identifies the manager as `CS::CSSfxImp` (vtable RVA `0x2bc31a0`, singleton slot
`0x3d87a28`) and the allocated cap as `CS::FfxbndFileCap` (vtable `0x29daae0`, size `0x158`).
The full copy chain between stored `ChrRes+0x170` and resource-state `+0x1c0` is not yet traced.

The bundle processor recognizes UTF-16 extension strings at RVAs `0x29dabd0` (`tpf`),
`0x29dabd8` (`flver`), `0x29dabe8` (`anibnd`), `0x29dabf8` (`ffxreslist`) and `0x29dac10`
(`fxr`). Separate loops populate child-resource vectors and associate FXR/resource-list data.
This is evidence of whole-bundle processing, not merely a raw file read. GPU readiness and actual
effect rendering still require a live test.

The alias ends in `.ffxbnd`; the native file layer is expected to resolve the physical compressed
bundle. Loose-file/mod-path precedence and `.dcx` resolution must be verified, not bypassed with
an invented absolute-path loader.

## Lifetime Findings

Bank acquire/release candidate ABI is a manager pointer plus a pointer to an int32 Bank ID.
The acquire wrapper handles an optional devpatch path as well as the ordinary path.
This is inferred from disassembly, not a type-safe exported interface.

Path acquisition increments FD4 cap reference count `+0x58` and bundle-use count `+0x150`.
On first insertion, a manager baseline and a caller use are added; repeated acquisitions add a
caller use. Bank release reduces the use count and delegates to CSFile vtable `+0x30`.
Do not substitute direct destruction or overwrite the counts.

The manager collector attempts cleanup only when:

1. File state at `+0x88` equals 4.
2. Bundle-use count at `+0x150` is at most 1.
3. Guard `0x20f2c0` finds no FXR child with a positive counter at `+0x7c`.

It then releases the manager baseline and only erases its list node when the CSFile lookup no
longer finds that cap. The positive child counter is consistent with active-resource use, but its
complete increment/decrement contract has not been established.

Inference: the DLL should acquire one native reference per demanded Bank and release only that
reference after the last configured player condition ends, leaving native cleanup to the game.
This may solve shared Boss/player ownership without the Wwise-specific already-loaded rule.
It is not yet proof of safe behavior for every map transition or failure path.

Important: readiness helper `0xd90b80` returns true even when the cap is missing. A successful probe
must establish cap existence, completed processing and actual usable FXR, not only that boolean.

## Primary Source Cross-checks

The pinned [FSRS resource definitions](https://github.com/vswarte/eldenring-rs/blob/902851865d05069eda7dcfd6385c8881eadf29bf/crates/eldenring/src/fd4/resource.rs)
name Ready as state 4 and declare the FD4 reference count as u32, consistent with native dword
increments. The [CSSfx definition](https://github.com/vswarte/eldenring-rs/blob/902851865d05069eda7dcfd6385c8881eadf29bf/crates/eldenring/src/cs/sfx.rs)
exposes the singleton layout but not this Bank API.

[libER's file wrapper](https://github.com/Dasaav-dsv/libER/blob/d8ae5c92719adca4344d8d902b63286b807f562b/source/coresystem/file.cpp)
maps FFXBND loading to a native factory and schedules file requests on FileStepUpdate_Begin.
Its old addresses, generic call signature, forced reference count and direct destruction are not
a validated ownership model for this DLL. Scheduling is a lead to investigate, not an established
task contract for the CSSfx Bank wrappers.

## Proposed Human Probe, Not Yet Approved

Supply one Bank ID, an FXR contained in it and an available weapon category or SpEffect trigger.
Use a fresh offline session in an area without its source character or another carrier. Record
whether any other mod already makes the FXR available. Keep the existing released DLL available
for rollback; do not include experimental SFX code in a public release yet.

- Cold load: demonstrate the FXR is unavailable before acquisition, then visible with all required
  textures/models after the Bank finishes loading, without spawning its source character.
- Trigger union: weapon or SpEffect keeps one request; ending one trigger does not release while
  another player/trigger still requests it.
- Shared ownership: with a native source character present, ending the DLL trigger must not break
  that character's effects. This is required before enabling automatic release by default.
- Live effect: end the trigger during a persistent effect; it must finish without disappearing,
  invalid textures or a crash. Final physical collection can be deferred by native ownership.
- Repeat/failure: perform ten load/release cycles; a missing Bank must report failure without
  growing references, claiming successful registration or affecting the sound-only path.
- Proposed timing: log request/ready/release transitions; allow 10 seconds for diagnostic loading,
  then report timeout without assuming cancellation. Immediate first-frame SFX is excluded from
  the initial test; prewarm and only trigger playback after readiness is established.
- Follow-up topology: repeat with two Seamless Co-op clients. Resource loading is local to each
  client; observed remote conditions, not a single client's loaded memory, must drive each DLL.

Open implementation questions: valid game task/lock context, completion/failure states, exact
reference balancing on failed or repeated requests, duplicate FXR IDs, world teardown and safe
version/signature resolution. Runtime Spec/Probe confirmation and human final acceptance remain
required; these static results are not a PASS for runtime support.
