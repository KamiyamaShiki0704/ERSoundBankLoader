---
id: FIX-SharedSoundBank
type: spec
version: 1.0
updated: 2026-10-18
source: human-raw
---

# Preserve Native Character Sound Banks

This bug fix restores the previously requested behavior: switching away from a mapped weapon
must not unload a Bank also used by the game's characters. The 2026-10-18 report supplies the
weapon-first, corresponding-Boss-present reproduction. This is an existing requirement repair,
not a new configurable loading mode. Current task state lives only in TASK_STATUS.md.

## Behavior

- SPEC-SOUND-001: When a native character needs a tracked Bank, the DLL shall preserve that
  Bank when the last configured weapon or SpEffect demand ends, regardless of load order.
- SPEC-SOUND-002: Once native character demand has been observed for a tracked load, the DLL
  shall relinquish permission to unload that load, including after the character disappears.
  Game resource cleanup may subsequently unload it; preservation is not a permanent preload.
- SPEC-SOUND-003: While native demand cannot be completely inspected, the DLL shall defer
  unloading its own inactive Banks and retry inspection on subsequent condition polls.
- SPEC-SOUND-004: When only configured triggers have used a Bank and inspection is complete,
  the DLL shall retain normal dynamic unload behavior. Pre-existing external Banks remain protected.
- SPEC-SOUND-005: Native character Bank selection shall include primary and additional Banks,
  with the primary character default used when its selection is negative. Zero selects no Bank.

## Semantic Probes

| Case | Preconditions and input | Expected output |
| --- | --- | --- |
| P1 Late native use | Weapon loads Bank; native Boss needs it; switch away | Boss Bank remains available |
| P2 Early native use | Boss Bank exists first; activate and deactivate weapon/SpEffect | No DLL unload of external Bank |
| P3 Remember use | Observe Boss while weapon stays active; Boss leaves; switch away | DLL still skips unload for that tracked load |
| P4 Ordinary dynamic | No native user; activate then deactivate last trigger | One unload of DLL-owned Bank |
| P5 Incomplete inspection | Last trigger ends while scene data is unavailable; later recover | Defer, then resume normal decision |
| P6 Bank selection | Explicit, default, additional, zero and duplicate selections | Correct conservative set; no inferred negative Bank ID |

Automated probes model the synchronous Sound Bank API and exercise the production ownership and
unload path. The inspection poll cadence remains poll_every_frames (default six frames); no new
timeout or grace period is introduced. Manual acceptance repeats P1/P2 with the user's matching
weapon and Boss, verifies normal Boss actions after switching, and checks weapon-only switching.
Short native lifetimes entirely between polls, non-character Bank users and native loader races
are not proven by the automated fixtures. SFX behavior, active configuration and game installation
are outside this fix's modification scope.
