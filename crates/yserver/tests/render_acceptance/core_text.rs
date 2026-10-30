use super::*;

// ── Core text through a GC created without GCFont ──────────────
//
// Xorg gives every new GC the server default font, "fixed"
// (`dix/gc.c:507`), so XCreateGC + XDrawString draws in it. Driven
// through `process_request` so the GC's font reaches the backend the
// way a client's does.

const CT_W: u16 = 48;
const CT_H: u16 = 20;
const CT_FG: u32 = 0x0000_ff00;

fn ct_window(f: &mut ProtoFixture, wid: u32) {
    let root = yserver_core::resources::ROOT_WINDOW.0;
    // CWBackPixel = black, so any non-black pixel is ink.
    or_create_window(f, wid, root, 0, 0, 0, CT_W, CT_H, 0, 0, 0x2, &[0]);
    f.req(8, 0, &wid.to_le_bytes());
}

/// PolyText8 of `text` at (2, 14), one TEXTITEM8, zero-padded.
fn ct_poly_text8(f: &mut ProtoFixture, drawable: u32, gc: u32, text: &[u8]) {
    let mut body = Vec::new();
    body.extend_from_slice(&drawable.to_le_bytes());
    body.extend_from_slice(&gc.to_le_bytes());
    body.extend_from_slice(&2i16.to_le_bytes());
    body.extend_from_slice(&14i16.to_le_bytes());
    body.push(u8::try_from(text.len()).expect("len"));
    body.push(0);
    body.extend_from_slice(text);
    body.resize(body.len().next_multiple_of(4), 0);
    f.req(74, 0, &body);
}

fn ct_ink(f: &mut ProtoFixture, wid: u32) -> Vec<u8> {
    let (_, _, px) = f.backing(wid);
    px
}

fn ct_has_ink(px: &[u8]) -> bool {
    px.chunks_exact(4).any(|p| p[..3] != [0, 0, 0])
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn fontless_gc_poly_text8_draws_in_the_default_font() {
    const WID: u32 = 0x0070_0001;
    const GC: u32 = 0x0070_0002;
    let Some(mut f) = ProtoFixture::new() else {
        eprintln!("skipping: no Vk");
        return;
    };
    ct_window(&mut f, WID);
    or_create_gc(&mut f, GC, WID, CT_FG);
    ct_poly_text8(&mut f, WID, GC, b"WW");
    let px = ct_ink(&mut f, WID);
    assert!(
        px.chunks_exact(4).any(|p| p[..3] == or_bgr(CT_FG)),
        "a GC without GCFont must draw in the default font"
    );
}

/// The font another GC selected must not leak into a GC without one.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn fontless_gc_ignores_a_font_set_on_another_gc() {
    const REF: u32 = 0x0071_0001;
    const OTHER: u32 = 0x0071_0002;
    const AFTER: u32 = 0x0071_0003;
    const PLAIN_GC: u32 = 0x0071_0010;
    const FONT_GC: u32 = 0x0071_0011;
    const CURSOR_FONT: u32 = 0x0071_0020;
    let Some(mut f) = ProtoFixture::new() else {
        eprintln!("skipping: no Vk");
        return;
    };
    for wid in [REF, OTHER, AFTER] {
        ct_window(&mut f, wid);
    }
    or_create_gc(&mut f, PLAIN_GC, REF, CT_FG);
    ct_poly_text8(&mut f, REF, PLAIN_GC, b"WW");

    // OpenFont("cursor"), then a GC holding it: GCForeground | GCFont.
    let mut body = Vec::new();
    body.extend_from_slice(&CURSOR_FONT.to_le_bytes());
    body.extend_from_slice(&6u16.to_le_bytes());
    body.extend_from_slice(&[0, 0]);
    body.extend_from_slice(b"cursor\0\0");
    f.req(45, 0, &body);
    let mut body = Vec::new();
    for word in [FONT_GC, OTHER, 0x0000_4004, CT_FG, CURSOR_FONT] {
        body.extend_from_slice(&word.to_le_bytes());
    }
    f.req(55, 0, &body);
    ct_poly_text8(&mut f, OTHER, FONT_GC, b"WW");
    ct_poly_text8(&mut f, AFTER, PLAIN_GC, b"WW");

    let reference = ct_ink(&mut f, REF);
    let other = ct_ink(&mut f, OTHER);
    assert!(ct_has_ink(&reference), "the plain GC drew nothing");
    assert!(ct_has_ink(&other), "the cursor-font GC drew nothing");
    assert_ne!(
        reference, other,
        "the two fonts must differ to prove anything"
    );
    assert_eq!(
        ct_ink(&mut f, AFTER),
        reference,
        "the plain GC drew in the other GC's font"
    );
}
