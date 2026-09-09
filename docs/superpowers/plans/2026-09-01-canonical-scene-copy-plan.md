# Canonical scene copy plan

1. Add a per-output device-local canonical scene target, with explicit image
   layout tracking and lifecycle teardown/recreation alongside output rebuilds.
2. Split the existing compose recorder into canonical-scene rendering and
   canonical-to-scanout rectangle copy recording. Preserve fence and
   renderer/KMS ownership sequencing.
3. Replace scanout BO generation history with per-BO pending-copy damage.
   Fan each canonical update out to all BOs; retire/clear a BO's submitted
   damage only after its successful page flip.
4. Implement conservative recovery paths and unit-test fan-out, failed flips,
   and output rebuilds.
5. Plumb `CWBitGravity` into the KMS window projection and add a transaction
   boundary around resize storage replacement; synchronize the old paint batch
   before preserving gravity-selected pixels.
6. Add resize-region tests for Forget/NorthWest/Center/SouthEast gravity,
   grow/shrink, repeated in-flight configures, and allocation/copy rollback.
   Verify core's existing Expose fanout is not doubled.
7. Smoke-test MATE menus/decorations and interactive resize, then run
   Awesome/mpv telemetry on hardware before considering merge.
