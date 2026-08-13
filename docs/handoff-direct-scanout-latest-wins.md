# Handoff — direct-level latest-wins supersession (feat/direct-scanout-latest-wins)

Fecha: 2026-08-12 (noche). Estado: **spec + plan completos, revisados adversariamente en 2 rondas. SIN implementar todavía.**

## Ramas

- **`fix/fullscreen-novsync-stutter`** @ `9d44b6be` — el fix del stutter listo para squash-merge.
  Contiene: Phase A (async defer+supersession, 22674c7e/ca88a14b/225551f4) + fix C1
  (degradar unflip, 4c98c3fd) + Phase B gates (6182aa52/37bd6c0c/2d7ab515) + fmt (3fa1ec7c)
  + docs de validación (9d44b6be). **Pendiente:** decisión de squash-merge (el usuario
  decidió avanzar con el feature antes de mergear).
- **`feat/direct-scanout-latest-wins`** @ `237aea3d` — el feature NUEVO.
  Hereda todo el fix + 4 commits extra:
  - `b09ca26b` log one-shot del m1-guard decline
  - `6d729597` diagnosis: NVIDIA deshabilita cursor HW (scene.rs:644)
  - `6dd4384f` override `YSERVER_HW_CURSOR_NVIDIA=1` (A/B lever)
  - `6f5af3a4` spec
  - `bda7a0ac` spec tras ronda adversarial 1
  - `af7c5b92` plan
  - `237aea3d` plan tras ronda adversarial 2

## Problema que resuelve

Phase B (direct scanout) **thrashea** en el box NVIDIA con CS2 no-vsync: ~3.7 unflips/s,
`page_flip/s` cae de 60 a 54-56. Causa raíz doble:
1. `present_flip_in_flight()` = `scene.has_pending_page_flips()` NO ve el flip directo
   (`scanout_m2.pending`), que retira via `retire_direct_output` sin tocar pending_acks
   de la escena → el core no parkea los presents synced → llegan al direct path con
   `pending.is_some()` → Copy+unflip.
2. No hay supersession a nivel directo: cada present que pilla el flip en vuelo desarma
   el directo.

## El fix (spec `docs/superpowers/specs/2026-08-12-direct-scanout-latest-wins-supersession-design.md`)

Dos piezas:
1. **Piece 1:** `present_flip_in_flight()` y `present_completion_is_idle()` suman
   `scanout_m2.pending.is_some()` → el core parkea/scrappe el flood (supersession synced
   existente) a ~1 present/flip antes del direct path.
2. **Piece 2:** slot `scanout_m2.queued` (latest-wins directo) + chain-flip diferido:
   un present que llega con flip en vuelo reemplaza el `queued` (Skip en orden), y
   `retire_direct_output` promueve `queued→pending`; `maybe_composite` submittea el
   frame promovido el próximo tick.

## El plan (`docs/superpowers/plans/2026-08-12-direct-scanout-latest-wins-supersession.md`)

7 tasks TDD:
1. `present_flip_in_flight`/`present_completion_is_idle` ven el flip directo (+ campo
   `fb` en `DirectPresentFrame` + constructor `for_tests`).
2. Cache m1 guarda `Arc<DirectScanoutProbeFramebuffer>` (evita doble-free / handle dangling).
3. Queued-store branch en `try_present_direct` + `prepare_direct_frame`/`complete_queued_as_skip`.
4. Chain-flip promotion en `retire_direct_output` + gate de `maybe_composite` + submit.
5. Test de Skip-ordering en yserver-core (regresión PIN, no failing-first).
6. Validación hardware (nvidia, `YSERVER_HW_CURSOR_NVIDIA=1`, CS2 ~3 min).
7. CI gate + squash-merge.

## Rondas adversariales (ambas aplicadas)

- **Spec, ronda 1:** 3 Critical (gate de maybe_composite traga el chain-flip; estado stale
  de `request_direct_unflip` en queued-store; `fb_handle` dangling) + Important #4
  (`present_completion_is_idle` no ve pending). Corregidos + tabla de transición agregada.
- **Plan, ronda 1:** 6 blockers (B1 gate, B2 retire destruía frame retirado, B3 Arc en cache,
  B4 `crate::backend`+`client_id`, B5 `fb:None` en production site, B6 failure contract)
  + I1-I3.
- **Plan, ronda 2:** B1-B6/I1-I2 verificados cerrados; I3 test seguía roto (assertion
  `present_id==0` errónea) → corregido; blocker NUEVO (borrow conflict de `fb_ref` → clonar
  a owned `Arc`); 2 Important nuevos (phantom-retire guard `if !pending_is_submitted { return false; }`;
  `pending_is_submitted` reset en stop_direct); spec gap (test Skip-ordering en yserver-core
  → nueva Task 5); minors (cursor_bound_all en tests del fixture, prepare_direct_frame
  infalible sin Result, call sites teardown).

## Puntos de código clave

- `crates/yserver/src/kms/render/backend.rs:13627` — `present_flip_in_flight`.
- `:7096` — `present_completion_is_idle`.
- `:13212` — el gate que se reemplaza (queued-store).
- `:1507` — `retire_direct_output` branch (promoción).
- `:12911-12914` — gate de `maybe_composite` (chain submit).
- `:1305` — `stop_direct_after_scanout_replaced` (clear queued + reset pending_is_submitted).
- `:252` — `ScanoutM1ProbeEntry` (pasa a Arc).
- `:317` — `ScanoutM2State` (fields `queued`, `pending_is_submitted`).
- `crates/yserver/src/kms/render/scene.rs:639-644` — override NVIDIA (no tocar, out of scope).
- `crates/yserver-core/src/core_loop/process_request.rs:8602` — `successor_presents_full_extent`.

## Validación conocida (para contexto de Task 6)

- CS2 no-vsync, nvidia box: `options=0x8` = PresentOptionSuboptimal (synced). Supersession
  synced coalesce ~266 skips/s sin directo. Directo SIN el fix: 1984 submits + 553 unflips,
  page_flip 54-56. El override NVIDIA habilita `cursor_hw`.
- Aceptación Task 6: page_flip ~60 sostenido, unflips ~0 en estado estable, 0 request_exit,
  0 chain-submit failures.

## Pendientes

1. Ejecutar el plan (subagent-driven o inline) — 7 tasks TDD.
2. Decidir squash-merge del fix (`fix/fullscreen-novsync-stutter`) — el usuario eligió
   avanzar el feature primero; el fix queda listo para mergear.
3. Validación hardware del feature (Task 6) requiere que el usuario juegue CS2 ~3 min.

## Notas

- El `.superpowers/sdd/2026-08-11-fullscreen-direct-scanout/progress.md` es del plan ANTERIOR
  (el fix) — ese plan está completo; este handoff es para el feature nuevo.
- Juego real del usuario: Marvel Rivals (UE5/RADV), no CS2. CS2 es el stand-in reproducible.
