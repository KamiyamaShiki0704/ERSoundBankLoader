---
id: FEAT-ConditionalSfxBank
type: design
version: 2.0
updated: 2026-09-05
based_on:
  - spec.md@2.0
---

# Opt-in Native SFX Experiment

## Boundary

Keep native SFX in one internal module. Reuse all-player weapon/SpEffect detection and task
registration without changing Wwise ownership behavior. Default SFX is off; public default INI
remains generic. Repeated weapon and SpEffect mappings resolve to one deduplicated Bank-demand set.
The old experimental enable key remains an alias. Devpatch support remains out of scope.
No new Cargo dependency or upstream revision change is required.

FrameBegin publishes only Bank demand through a short-lived mutex. Resource work runs on
FileStepUpdate_Begin, the file-request stage used by the libER reference. This is an experimental
scheduling choice, not a proof covering arbitrary mods or every game task race; native task
association/concurrency evidence must be reviewed before delivery. Neither startup workers nor
SoundStep call SFX functions. A mutex between our tasks does not lock the game's own manager.

## Native Adapter

Validate PE timestamp, image size and full code-range fingerprints for the inspected 2.7.0.0
acquire/release/factory/reference/collector functions before exposing an adapter. Hashes are
accidental-change guards, not cryptographic authentication. Any mismatch disables SFX only.
Check CSSfx and CSFile vtables, required non-null globals and the bundle-list sentinel at runtime.

Call CSSfx path acquisition (RVA d90cf0) with the native alias
`sfxbnd:/SfxBnd_c####.ffxbnd`, null auxiliary argument and false flag. It returns the exact
FfxbndFileCap for the caller's reference. Release that cap through d91020, never by broad Bank-ID
release (which also touches devpatch) and never by direct destruction or hand-written decrements.

Before reading a retained cap, verify it remains in the bounded CSSfx list and has the expected
FFXBND vtable. Capture the manager/file/sentinel identity with each handle. If identity changes or
ownership is unverifiable, stop experimental calls until restart rather than releasing stale
pointers. Both file and bundle counts must retain the manager baseline before releasing our use.
This may conservatively retain a reference in an anomalous session; report that explicitly.

Observed CSFile vtable +30 -> 26560a0 -> 2654170 -> 2654210 decrements cap+58, and only removes
the cap at zero with cap+5c <=0. Evidence: dist/sfx-recon/csfile-reference-release.json. Pending
requests can therefore release our extra use while retaining the native manager baseline.

## Demand State and Diagnostics

One record per Bank: acquired handle, request time, ready/timeout notification flags. A null
acquisition is latched until demand ends. A pending/timeout handle is never reacquired while
demand remains. A timeout is diagnostic, not cancellation. Ended demand releases exactly once;
physical collection remains native. Invalid identity or unsafe count observations latch SFX off.

Ready observation requires a retained cap in state 4, valid nonempty FXR vector, and expected
FFXBND identity. Record dependent vector counts; call this `resources_ready`, never visual PASS.
Readiness helper d90b80 is intentionally unused because it treats missing caps as ready.

Log SFX setup, condition transitions, acquisition identity/counts, resources_ready with child
counts, timeout at 10 seconds, release and fail-closed diagnostics. Honor existing log_enabled.
No logs every poll. Pause world transitions with unavailable managers; once a live handle exists,
loss of its manager is a fail-closed condition requiring restart.

## Verification and Delivery

- SPEC-SFX-001/002: fake-backend tests for category mapping, idempotent polling and shared demand.
- SPEC-SFX-003: fake reference owners and pending-release tests; native code fingerprint test.
- SPEC-SFX-004: timeout, missing request, no retry storms and lost-identity tests.
- SPEC-SFX-005: existing sound tests, default-off parsing and unchanged deployment hashes.
- Run Rust fmt, unit tests, strict Clippy, release build, existing ER compatibility probe and
  opt-in SFX executable fingerprint probe. A skipped executable probe is not a PASS.
- Deliver separate ignored dist experiment directory and ZIP with generic instructions and the
  selected experimental config. Keep live DLL/INI, public version/tag and Git remote unchanged.
- P1-P5 visual/game tests remain manual per spec 1.0. No UE/Recorder implementation is available
  in this native ER project; report automated runtime Probe coverage as unavailable, not PASS.
