---
id: FIX-SharedSoundBank-Design
type: design
version: 1.0
updated: 2026-10-18
source: ai-derived
---

# Native Demand and Unload Ownership

Input: spec.md@1.0, SPEC-SOUND-001..005. Base revision: 86bb27cc6f01cc4497d3192e1ca5646efd6418d6.

The existing initial AKRESULT distinguishes a pre-existing Wwise load from a DLL-created one.
It cannot report a later native caller: duplicate explicit loads do not create a distinct owner
token. The regression drives the production unload function with a fake Wwise Bank and confirms
that the weapon-first/native-later sequence currently unloads the Bank.

Before inactive unload on each existing FrameBegin poll, read native character requirements
through the pinned WorldChrMan character sets and SoloParamRepository NpcParam rows. Enumerate
the registered sets plus explicit map, summon, debug and player sets; deduplicate identities.
Do not filter dead, inactive or distant characters: their resources may still be needed.
Collect only Banks for which the DLL currently holds unload permission.

NpcParam.sound_bank_id and sound_add_bank_id are signed i16 fields at 0x248 and 0x2a4. On
ER 2.7.1.0 (SHA-256 1a3547101327f65d0c76da2f9190ac0aa66871ea42bae2aecc61e11a8b597891),
0x4cf0bd and 0x4cf0cf read both; leaf setter 0x3f6230 stores them in character resource parameters.
Native resource initialization at 0x4cbfbe..0x4cbfd7 selects a nonnegative primary override, or the
character default otherwise; 0x4ccadd..0x4ccae5 skips a zero primary Bank. Positive additional
selection is protected conservatively; its complete native activation path is not established.
These RVAs are evidence, not new native function calls or runtime constants in this fix.

Represent an inspection as Bank IDs plus completeness. Partial known results can transfer
ownership even when other character data is missing. Unknown world/repository/row data defers
remaining DLL-owned unloads. A native-character ownership state is sticky for that tracked load;
inactive tracking may be released without calling Wwise. This observes demand, not an actual
Wwise ownership-query API, and may preserve a Bank longer than strictly necessary.

P1/P2/P3 use production unload with the fake API and both load orders. P4 checks real fake-unload
side effects. P5 checks incomplete-to-complete recovery and partial protection. P6 checks Bank
selection independently. Compatibility tests still run against the current EXE, with unchanged
FSRS pin and SFX ABI guard. Manual P1/P2 remain the authority for audible Boss behavior.

No live debug hook or Recorder runtime is available for this native ER DLL. Evidence consists of
Rust results, bounded native instruction references and a local package manifest. Deliver one
separate generic package, then remove only this run's unused compiler intermediates and staging
copies; keep the final package, DLL and reproducible source/tests.
