# Handoff — fijar el mecanismo del bug de transparencia con un test

Fecha: 2026-08-11. Estado: investigación pausada al 90% de la ventana de tokens.
Retomar AQUÍ. Findings completos en
`docs/superpowers/findings/2026-08-11-yserver-leak-cinnamon-regression-and-transparency-bug.md`.

## Contexto en una línea

El leak-fix (`24d97cd6`, solo en `origin/diag/115-resource-population`) libera
pixmaps huérfanos de GC sin consultar referencias de Render Pictures; si el
backing no está materializado en el store cuando la Picture se crea, el
orphan-release puede destruir un drawable que una Picture sigue usando →
ventana transparente al arrancar un juego.

## El mecanismo a fijar (hipótesis confirmada por análisis estático)

Cadena causal:

1. `handle_free_pixmap` (`yserver-core/src/core_loop/process_request.rs`,
   ~25633 en el working tree) solo retiene el host pixmap si
   `host_xid_referenced_by_window_bg || host_xid_referenced_by_gc`. **No
   consulta `pictures`.**
2. En la rama leak-fix, `orphaned_host_pixmaps`
   (`yserver-core/src/resources.rs` en `24d97cd6`) se ejecuta en
   `ChangeGC`/`CopyGC`/`SetClipRectangles`/`FreeGC`/XFixes `SetGCClipRegion`.
   Libera el clip/tile/stipple del GC cuando ya nadie (pixmap resource,
   window-bg, otro GC) lo referencia. **Tampoco consulta `pictures`.**
3. El único respaldo es el refcount del KMS store:
   `render_create_picture` (`kms/render/backend.rs`, ~17312 working tree) hace
   `if let Some(id) = self.store.lookup(drawable_xid) { self.store.incref(id); ... }`.
   Ese incref es **condicional a que el drawable ya esté en el store al
   momento de crear la Picture**.
4. **El hole a probar**: si la Picture se crea sobre un pixmap cuyo backing aún
   no está en el store (map+redirect antes de la alloc, o GLX-TFP / Present /
   DRI3 import), la Picture NO tiene store ref; un orphan-release posterior de
   un GC llama `backend.free_pixmap` → `store.decref` llega a cero → drawable
   destruido bajo la Picture → transparente.

## Qué hay que probar (test de workspace)

Objetivo: **fijar que el orphan-release puede liberar un host_xid que una
Picture sigue referenciando**, y que el refcount del store NO protege cuando la
Picture se creó antes de que el backing estuviera en el store.

### Test 1 (core + recording backend, en `yserver-core`)
- Crear pixmap P (host_xid H), GC con clip/tile/stipple=H, y una Picture sobre P.
- `FreePixmap(P)` → H retenido (GC lo referencia). ✓ (ya cubierto)
- `FreeGC`/`ChangeGC` → con la rama leak-fix, `orphaned_host_pixmaps` devuelve H
  y se llama `backend.free_pixmap(H)`.
- **Aserción clave**: si la Picture todavía existe, el free NO debería ocurrir
  (o al menos el store debería retener el drawable). Con el código actual de la
  rama leak-fix, `free_pixmap` se llama pese a la Picture → test falla → bug
  fijado.
- El test existe parcialmente: `free_gc_releases_pixmap_retained_after_free_pixmap`
  en `process_request.rs` (leak-fix). Falta la variante CON Picture.

### Test 2 (orden picture-antes-de-backing, en `kms`)
- Verificar que `render_create_picture` sobre un xid sin entry en el store NO
  toma incref (`picture_drawable_ids` vacío), y que luego
  `store.allocate` + `free_pixmap` llega a refcount 0 aunque la Picture siga
  viva. Aserción: `store.lookup` tras el free devuelve None pero la Picture
  sigue en `core.pictures`.

### Comandos de verificación
```
cargo test -p yserver-core --lib <nombre_test>
cargo test -p yserver --lib
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt
```

## Cómo retomar

1. `git fetch` y revisar la rama: `git log origin/diag/115-resource-population`.
2. El working tree actual (`dri3-syncobj-drm-signal`) **NO tiene**
   `orphaned_host_pixmaps` (grep devuelve 0). Para el Test 1 hay que escribir
   el test contra el código de la rama leak-fix (`git show
   24d97cd6:crates/...`) o en un worktree de esa rama, porque es donde existe el
   bug.
3. Si se quiere reproducir el bug en el working tree actual: el Test 1 no
   aplica (no hay orphan-release); en su lugar fijar el agujero PRE-existente:
   `handle_free_pixmap` libera H aunque una Picture lo referencie (ver
   `host_owned_pixmap`, siempre `None` en create paths — investigar por qué).

## Puntos de código exactos

- Leak-fix (el bug):
  - `24d97cd6:yserver-core/src/resources.rs` — `orphaned_host_pixmaps` (~1820),
    `change_gc` (1957), `set_clip_rectangles` (2168), `clear_gc_clip` (2179),
    `copy_gc` (2283), `free_gc` (2286).
  - `24d97cd6:yserver-core/src/core_loop/process_request.rs` —
    `release_orphaned_host_pixmaps` (~24398), `handle_free_pixmap` (~24542).
  - `24d97cd6:kms/render/backend.rs` — `render_create_picture` (~15748,
    incref condicional), `free_pixmap` (~13396).
- Working tree actual (referencia del agujero pre-existente):
  - `yserver-core/src/core_loop/process_request.rs` — `handle_free_pixmap`
    (~25633), `release_picture` path (`render_free_picture` + `free_pixmap` en
    ~1702).
  - `kms/render/backend.rs` — `render_create_picture` (~17312),
    `render_free_picture` (~17418), `free_pixmap` (~13396 area).
  - `kms/render/store.rs` — `incref`/`decref`/`lookup`, detach de xid en
    PendingFence.

## Notas de contexto del log (evidencia)

- Game start: 22:16–22:17, op55/op59/op60 a 2000–4000/s, windows 313→361,
  store →1112 drawables, GetImage (op73) 757 ms a las 22:17:25.
- La regresión de rendimiento (busy-loop full-redraw + stalls 1.033s) es
  estructural y está documentada en el findings; es un problema separado del
  bug de transparencia.
