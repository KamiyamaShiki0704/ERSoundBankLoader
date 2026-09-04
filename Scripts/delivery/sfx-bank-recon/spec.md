---
id: FEAT-ConditionalSfxBank
type: spec
version: 2.0
updated: 2026-09-05
source: human-raw
---

# Configurable Conditional SFX Banks

Version 2.0 promotes the successful weapon experiment into generic user configuration and adds
SpEffect triggers. The original category86/Bank2120 case remains the first runtime evidence.

Human-confirmed on 2026-09-05. Current work status is maintained only in the project-root
TASK_STATUS.md. Native implementation evidence is in `draft.md`.

## Inputs and Scope

User selected weapon special category 86 and Bank 2120 (`sfxbnd_c2120.ffxbnd.dcx`).
The user will select and exercise the weapon actions during manual acceptance. No FXR ID is
required before Build. An FXR ID is a test observation, not a required Bank-mapping input.

The first experiment is offline, single-player and opt-in. No carrier NPC is spawned by the DLL.
The resource file must be available through the active game/mod setup. Existing sound behavior
and private configuration must be preserved. The broader SpEffect/co-op feature remains intended,
but is not claimed as validated by this first weapon-only experiment.

## Behavior

- SPEC-SFX-001: When a monitored player's active weapon has special category 86, the loader shall
  request Bank 2120 without spawning a carrier character.
- SPEC-SFX-002: While any monitored player requests the same Bank, the loader shall retain its
  demand without accumulating an additional request on every polling cycle.
- SPEC-SFX-003: When no monitored player requests Bank 2120, the loader shall release only its own
  demand; it shall not force removal of resources still needed by native characters or live effects.
- SPEC-SFX-004: If loading fails or exceeds the diagnostic timeout, the loader shall report the
  failure/timeout rather than claim readiness; it shall not accumulate duplicate outstanding
  requests. Timeout does not imply native cancellation or safe destruction.
- SPEC-SFX-005: The experiment shall not change existing Sound Bank behavior or require immediate
  physical unloading when other owners still use the SFX Bank.
- SPEC-SFX-006: The user may configure repeated weapon-category and SpEffect-to-SFX-Bank mappings
  without changing source code. The loader shall request the union of both trigger types across all
  monitored players and deduplicate identical Bank IDs.
- SPEC-SFX-007: SFX configuration shall be independent from Wwise file directories and Sound Bank
  enable/unload settings. Invalid SFX configuration or an unsupported executable shall disable
  native SFX calls without disabling valid Sound Bank behavior.
- SPEC-SFX-008: The user may set a diagnostic readiness timeout from 1000 through 120000 ms. A
  timeout shall not cause repeated acquisition or forced cancellation while demand remains active.

## Confirmed Probes

The user confirmed this test scope and retained responsibility for action/visual testing.

| Case | Preconditions and input | Expected result |
| --- | --- | --- |
| P1 Cold load | Fresh offline session; no source character/carrier; chosen effect absent before request; switch to category 86 and play the selected action after loading | Effect becomes visible with correct textures/models, with no carrier spawn |
| P2 Release and repeat | End the weapon condition, restore it, repeat ten cycles | Demand ends and resumes; no duplicate outstanding requests, crash or loss of existing sound |
| P3 Native shared owner | A native character using Bank 2120 is present; acquire through category 86 and then switch away | Native character effects continue working; physical residency is allowed |
| P4 Live effect | A selected effect is still running when switching away | The effect finishes normally; no abrupt loss of dependencies or crash |
| P5 Failure | Test a controlled unavailable Bank in a separate fixture | Clear failure or timeout, no false readiness or growing outstanding requests |

Diagnostic load timeout: 10 seconds from request. Ten repeated demand cycles are required;
this is not a performance benchmark or a requirement to physically unload within 10 seconds.
No visual tolerance is assigned numerically: the human compares the chosen effect with its known
appearance. If no persistent effect is available, P4 remains unverified rather than marked PASS.

Excluded from initial acceptance: immediate first-frame effects, online play, Seamless Co-op,
SpEffect-triggered SFX, duplicate FXR-ID overrides, unsupported game versions and public release.
These exclusions limit this experiment, not the intended eventual feature.

## Human Acceptance

The user confirmed the no-carrier test scope, 10-second timeout, ten cycles and exclusions, and
will choose the action and area. Final visual/native-owner acceptance is manual; static analysis,
logs and successful compilation do not prove rendering correctness.
No live automated Probe framework or renderer is assumed available at this Requirements stage.
