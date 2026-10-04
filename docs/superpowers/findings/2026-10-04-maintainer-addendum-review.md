## Verdict

2 blocking, 1 major, 0 minor.

Coverage: INCOMPLETE

The addendum contains two demonstrated Xorg contract mismatches and omits one requested hardware scenario. This is a design-review result; it does not establish that implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Disposition |
|---|---|
| None | First review; check 1 skipped. The maintainer triage was checked for scope coverage, not treated as a prior adversarial review. |

## Findings

### Blocking

**B-1 — E1 requires window fields that hierarchy events do not contain**

[Addendum lines 141–147](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:141) correctly requires delivery for each matching selecting window, but incorrectly promises events “with their own window fields.”

At `xorg-server-21.1.24:Xi/xichangehierarchy.c:73–120`, Xorg constructs one hierarchy event and sends it through `SendEventToAllWindows`. The wire structure, `/usr/include/X11/extensions/XI2proto.h:892–909`, contains no window field.

Concrete failure: a client selects hierarchy notifications on two windows. An implementation satisfying E1’s window-field requirement must invent or repurpose bytes in the event, producing incompatible wire content. Correct delivery produces two copies without window identification. This conflicts with the main design’s requirement for corresponding XI hierarchy notifications and live-device descriptors ([spec lines 210–216](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-09-29-dynamic-xinput-device-registry-design.md:210)).

Smallest correction: retain delivery multiplicity, remove the window-field promise, and require tests to verify event count and valid wire content.

**B-2 — C2 incorrectly derives class provenance from the current `lastSlave`**

[Addendum lines 112–115](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:112) says that a master without a last slave reports its own ID in its classes.

Xorg stores provenance in the copied classes: `xorg-server-21.1.24:Xi/exevents.c:592,630` assigns the slave ID to valuator/button class records, and `Xi/xiquerydevice.c:278,326` serializes stored class source IDs. Separately, `dix/devices.c:489–492` clears `lastSlave` during disable; that operation does not reset the copied class provenance.

Concrete failure: slave 6 supplies the master’s classes, then is disabled. A3 correctly clears the master’s last-slave reference. C2 now requires queries to report master ID 2, although Xorg retains the copied class provenance until those classes are replaced. The implementation would conflate routing state with class metadata. The main design already distinguishes clearing last-slave references from changing/publishing classes ([spec lines 168–170](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-09-29-dynamic-xinput-device-registry-design.md:168), [210–218](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-09-29-dynamic-xinput-device-registry-design.md:210)).

Smallest correction: preserve provenance in each mirrored class independently of `lastSlave`. Specify the initial master-owned classes separately. Test query results after the supplying slave is disabled, before another slave supplies classes.

### Major

**M-1 — Acceptance omits the requested touchpad-and-desktop hardware scenario**

The maintainer explicitly requested a touchpad laptop with a grab-heavy desktop such as Cinnamon, XFCE or KDE ([triage line 35](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/findings/2026-10-04-pr198-maintainer-review-triage.md:35)). The addendum’s concrete hardware acceptance paths cover VT, mouse disable/enable and replug ([lines 199–201](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:199)), but omit that scenario.

Concrete consequence: acceptance can complete using a mouse while never exercising touchpad scrolling under the desktop’s actual grabs—the behavior affected by F1. The main design expressly requires preservation of grabs, scroll and existing touchpad properties ([spec lines 347–349](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-09-29-dynamic-xinput-device-registry-design.md:347)).

Smallest correction: explicitly carry T1 into supervised hardware acceptance, naming touchpad scrolling under an identified desktop’s grabs and the expected delivery behavior.

### Minor

None established.

## Coverage and implementation checks

- **Check 1 — incorporation:** skipped because no prior review exists.
- **Check 2 — architecture and contracts:** assessed lifecycle publication ordering, query/class ownership, hierarchy delivery, shared libinput availability and the stated VT configuration contract. Current branch execution-loop integration was not inspected before the reading budget expired.
- **Check 3 — safety and failure semantics:** verified Xorg’s release-before-disable ordering, clearing of last-slave references, attached Disabled descriptors, shared-device shutdown conditions, unknown XTEST target rejection and relevant XKB lock-release semantics. G’s completion handoff across the actual core/input-thread boundary remains unassessed.
- **Check 4 — specification and verification:** compared relevant ownership, lifecycle, property, query and acceptance requirements. Verified the targeted Xorg paths supporting grab detachment, per-event owner-events fallback and property availability. Found the missing T1 acceptance scenario.

**Excerpts used: 24/24:** four main-spec excerpts, one triage excerpt, sixteen tagged Xorg excerpts, two libinput-driver excerpts and one protocol-header excerpt. Disjoint source ranges were counted separately. The addendum was read once; searches were bounded locators.

Unassessed questions include current branch behavior, reset persistence of client-disabled state, `Device Enabled` nonzero values and `GetProperty(delete)` semantics, cross-facet lifecycle interleaving, and the complete XTEST class/type details. These areas are not established sound.

A bounded follow-up should resolve those questions, especially whether G can finish submitted writes before yielding the VT without waiting for resume or rejecting an already-applied change.

No builds, tests, installs or compilation experiments ran. Implementation retains responsibility for real request/input-path tests, supervised hardware evidence, `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, and applicable portability checks.