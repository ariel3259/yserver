//! Glyph pixel sources for the `CompositeGlyphs` path and their
//! conversion to the dense A8 bitmap the atlas uploader consumes.
//!
//! The backend's wire parse forwards a glyph's pixels *as stored in
//! the glyphset* — dense A8 for `PICT_a8`, the raw A1 wire bitmap
//! for `PICT_a1`, the raw ARGB32 wire bytes for `PICT_a8r8g8b8`.
//! Conversion to A8 is **deferred to the engine's atlas-miss
//! branch** via [`GlyphPixels::to_a8`]: a glyph already resident in
//! the GPU atlas is never converted again, mirroring Xorg/EXA's
//! convert-on-upload (`exa/exa_glyphs.c`). See
//! `docs/superpowers/findings/2026-07-08-xorg-render-optimization-gaps.md` #2.

use std::borrow::Cow;

/// Pixel source for one glyph, as stored in its glyphset. Cheap to
/// carry (two `&[u8]` variants); the actual A1→A8 expansion only
/// happens on an atlas miss.
#[derive(Copy, Clone, Debug)]
pub(crate) enum GlyphPixels<'a> {
    /// Dense A8 coverage bitmap, row-major `w × h`. Native
    /// `PICT_a8` glyphs.
    A8(&'a [u8]),
    /// Raw A1 wire bitmap: rows padded to a 32-bit scanline unit,
    /// bit order = advertised `bitmap-bit-order` (LSBFirst on the
    /// common little-endian client). Expanded lazily by [`Self::to_a8`].
    A1Wire(&'a [u8]),
    /// Raw ARGB32 wire bytes, row-major `w × h` CARD32 pixels with
    /// no per-row padding (stride `4 * w`). **Memory order per pixel
    /// is `[B, G, R, A]`** — little-endian CARD32 with alpha at bits
    /// 24-31, so blue is the LOW byte. Reduced to a single A8
    /// coverage plane by [`Self::to_a8`] via
    /// [`reduce_argb32_glyph_to_a8_coverage`]; the four channels are
    /// kept intact this far so the component-alpha path can pack all
    /// of them (design
    /// `2026-09-10-component-alpha-glyphs-design.md`, stage 0).
    Argb32Wire(&'a [u8]),
}

impl<'a> GlyphPixels<'a> {
    /// Produce the dense `w × h` A8 bitmap the atlas uploader copies
    /// into its staging buffer. `A8` borrows in place (truncated to
    /// `w × h`); `A1Wire` expands and `Argb32Wire` reduces into an
    /// owned buffer. Returns `None` if the source is too short for
    /// the declared dimensions (caller drops the glyph), so a
    /// malformed request can never index out of bounds during
    /// upload.
    pub(crate) fn to_a8(self, w: u32, h: u32) -> Option<Cow<'a, [u8]>> {
        let cells = (w as usize).checked_mul(h as usize)?;
        match self {
            GlyphPixels::A8(src) => {
                if src.len() < cells {
                    return None;
                }
                Some(Cow::Borrowed(&src[..cells]))
            }
            GlyphPixels::A1Wire(src) => {
                let wire_stride = (w as usize).div_ceil(32) * 4;
                let need = wire_stride.checked_mul(h as usize)?;
                if src.len() < need {
                    return None;
                }
                Some(Cow::Owned(expand_a1_glyph_to_a8(src, w, h)))
            }
            GlyphPixels::Argb32Wire(src) => {
                let need = cells.checked_mul(4)?;
                if src.len() < need {
                    return None;
                }
                Some(Cow::Owned(reduce_argb32_glyph_to_a8_coverage(src, w, h)))
            }
        }
    }
}

/// Reduce an ARGB32 glyph's wire bytes to a single A8 coverage
/// plane, row-major `w × h` at **logical width `w`** — the mean of
/// logical R, G and B.
///
/// **Wire byte order is `[B, G, R, A]`** per pixel: X RENDER
/// `PICT_a8r8g8b8` is a little-endian CARD32 with alpha-shift 24, so
/// blue is the LOW byte of the four and alpha the HIGH one. Reading
/// bytes 1..=3 (i.e. `[G, R, A]`) instead of 0..=2 is the trap here:
/// it errors nothing and paints plausible-looking text off by a
/// channel.
///
/// **Coverage comes from R, G and B, never from the alpha byte.** A
/// subpixel-AA client puts one coverage value per LCD subpixel in R,
/// G and B and leaves alpha at 255 across the whole glyph box —
/// measured on OpenJDK 25, where a 9×9 glyph had exactly one
/// distinct alpha value (255, all 81 pixels) against 28 distinct
/// `max(rgb)` values. Taking the alpha byte therefore yields a solid
/// `0xFF` rectangle, which is precisely the "text renders as blocks"
/// defect this reduction fixes.
///
/// Pure function of the wire bytes, so it is testable without a GPU
/// and needs no runtime switch to reach
/// (`feedback_no_feature_kill_switches`). Today it is the universal
/// treatment of an ARGB32 glyphset; a later step narrows its
/// condition to devices lacking `dualSrcBlend`, where the design
/// already promises grayscale AA (`vk/device.rs`), and packs all four
/// channels otherwise.
///
/// Rows short of `4 * w * h` bytes are left zero rather than
/// panicking; the caller length-checks first, so this is belt and
/// braces.
pub(crate) fn reduce_argb32_glyph_to_a8_coverage(wire: &[u8], w: u32, h: u32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    // Dense CARD32 rows: X RENDER AddGlyphs pads a glyph row to a
    // 4-byte boundary, which 4 bytes per pixel already satisfies.
    let stride = w * 4;
    let mut a8 = vec![0u8; w * h];
    for row in 0..h {
        let row_off = row * stride;
        if row_off + stride > wire.len() {
            break;
        }
        for col in 0..w {
            let px = row_off + col * 4;
            // [B, G, R, A] — blue is the low byte. See above.
            let b = u32::from(wire[px]);
            let g = u32::from(wire[px + 1]);
            let r = u32::from(wire[px + 2]);
            // Mean of the three logical colour channels, rounded to
            // nearest. The alpha byte at `px + 3` is deliberately
            // NOT read: it is 255 everywhere on a subpixel-AA glyph.
            a8[row * w + col] = u8::try_from((r + g + b + 1) / 3).unwrap_or(u8::MAX);
        }
    }
    a8
}

/// Expand a wire A1 glyph bitmap to dense A8 (`0x00` / `0xFF` per
/// pixel), row-major `w × h`.
///
/// Wire rows are padded to a 32-bit scanline unit; bit order is
/// LSBFirst (bit 0 of a byte is the leftmost of its 8-pixel group),
/// matching yserver's advertised `bitmap-bit-order` on x86. Reading
/// MSBFirst would mirror every 8-pixel run — the issue-#77 bitmap-font
/// regression the tests below guard.
pub(crate) fn expand_a1_glyph_to_a8(pixels: &[u8], gw: u32, gh: u32) -> Vec<u8> {
    let wire_stride = (gw as usize).div_ceil(32) * 4;
    let mut a8 = vec![0u8; (gw as usize) * (gh as usize)];
    for row in 0..(gh as usize) {
        let src_off = row * wire_stride;
        if src_off + wire_stride > pixels.len() {
            break;
        }
        for col in 0..(gw as usize) {
            let byte = pixels[src_off + col / 8];
            let bit = (byte >> (col & 7)) & 1;
            a8[row * (gw as usize) + col] = if bit != 0 { 0xFF } else { 0 };
        }
    }
    a8
}

#[cfg(test)]
mod tests {
    use super::{GlyphPixels, expand_a1_glyph_to_a8, reduce_argb32_glyph_to_a8_coverage};
    use std::borrow::Cow;

    #[test]
    fn to_a8_borrows_dense_a8_truncated_to_cells() {
        // A8 source longer than w*h (trailing padding bytes) borrows
        // in place, truncated to exactly the glyph cells — no copy.
        let src = [10u8, 20, 30, 40, 0xEE, 0xEE];
        let out = GlyphPixels::A8(&src).to_a8(2, 2).expect("fits");
        assert!(matches!(out, Cow::Borrowed(_)), "A8 must not copy");
        assert_eq!(&*out, &[10, 20, 30, 40]);
    }

    #[test]
    fn to_a8_expands_a1_wire_lsb_first() {
        // 8x1 wire byte 0b0000_0001 → col 0 set (LSBFirst). Owned copy.
        let wire = [0b0000_0001u8, 0, 0, 0];
        let out = GlyphPixels::A1Wire(&wire).to_a8(8, 1).expect("fits");
        assert!(matches!(out, Cow::Owned(_)), "A1 must expand into owned");
        assert_eq!(&*out, &[0xFF, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn to_a8_none_when_a8_too_short() {
        let src = [1u8, 2, 3];
        assert!(GlyphPixels::A8(&src).to_a8(2, 2).is_none());
    }

    #[test]
    fn to_a8_none_when_a1_wire_too_short() {
        // 8x2 needs two 4-byte scanline units; give only one.
        let wire = [0xFFu8, 0, 0, 0];
        assert!(GlyphPixels::A1Wire(&wire).to_a8(8, 2).is_none());
    }

    #[test]
    fn to_a8_none_when_argb32_wire_too_short() {
        // 2x2 needs 4 pixels × 4 bytes = 16; give 15.
        let wire = [0u8; 15];
        assert!(GlyphPixels::Argb32Wire(&wire).to_a8(2, 2).is_none());
    }

    // ── the ARGB32 → A8 coverage reduction ──
    // Subpixel/LCD-antialiased text used to render as one solid block
    // per glyph because ingest kept the ARGB32 **alpha** byte as the
    // coverage plane, and a subpixel-AA client leaves alpha at 255
    // across the whole glyph box.

    #[test]
    fn argb32_reduction_is_the_mean_of_logical_rgb_at_exact_values() {
        // Wire memory order is [B, G, R, A]: blue LOW, alpha HIGH.
        //
        // px0: B=0x20 G=0x60 R=0xA0 A=0xE0 → (0xA0+0x60+0x20)/3 = 96.
        // px1: B=0x10 G=0x40 R=0x50 A=0xFF → (0x50+0x40+0x10+1)/3 = 53.
        //
        // Exact values, not a "the channels differ" check: that would
        // pass with red and blue reversed. The two pixels together
        // also pin the reduction against the near misses —
        //   * reading bytes 1..=3 as if they were R,G,B (i.e. [G,R,A])
        //     gives 160 and 133, not 96 and 53;
        //   * taking the alpha byte gives 0xE0 and 0xFF;
        //   * taking any single colour channel gives one of the
        //     inputs — px1's mean 53 is not 0x10, 0x40 or 0x50, and
        //     px0's 96 rules out R and B.
        let wire = [0x20u8, 0x60, 0xA0, 0xE0, 0x10, 0x40, 0x50, 0xFF];
        let a8 = reduce_argb32_glyph_to_a8_coverage(&wire, 2, 1);
        assert_eq!(
            a8,
            vec![96u8, 53u8],
            "coverage must be the mean of logical R, G and B read from \
             wire bytes [B, G, R, A]"
        );

        // Same answer through the uploader's entry point, so the
        // engine's atlas-miss branch cannot be wired to a different
        // reduction than the one tested here.
        let via_to_a8 = GlyphPixels::Argb32Wire(&wire).to_a8(2, 1).expect("fits");
        assert!(
            matches!(via_to_a8, Cow::Owned(_)),
            "ARGB32 must reduce into an owned buffer"
        );
        assert_eq!(&*via_to_a8, &[96u8, 53u8]);
    }

    #[test]
    fn argb32_reduction_of_a_subpixel_aa_glyph_is_not_a_solid_block() {
        // Synthetic 9×9 glyph reproducing the *measured* signature of
        // OpenJDK 25's LCD glyphs (one vng run, temporary
        // instrumentation): alpha has exactly ONE distinct value, 255
        // over all 81 pixels, while max(R,G,B) spans dozens. These
        // are not the literal Java bytes — the instrumentation
        // recorded histograms, not the glyph — but the property that
        // makes the old code fail is the whole fixture.
        const W: usize = 9;
        const H: usize = 9;
        let mut wire = vec![0u8; W * H * 4];
        for row in 0..H {
            for col in 0..W {
                let px = (row * W + col) * 4;
                wire[px] = u8::try_from((row * 29 + col * 7) % 256).expect("masked");
                wire[px + 1] = u8::try_from((row * 13 + col * 23 + 5) % 256).expect("masked");
                wire[px + 2] = u8::try_from((row * 41 + col * 3 + 11) % 256).expect("masked");
                wire[px + 3] = 0xFF;
            }
        }

        // The fixture really does have the measured shape.
        let mut alphas: Vec<u8> = wire.iter().skip(3).step_by(4).copied().collect();
        alphas.dedup();
        assert_eq!(alphas, vec![0xFFu8], "fixture: alpha constant 255");
        let mut maxima: Vec<u8> = (0..W * H)
            .map(|i| wire[i * 4].max(wire[i * 4 + 1]).max(wire[i * 4 + 2]))
            .collect();
        maxima.sort_unstable();
        maxima.dedup();
        assert!(
            maxima.len() >= 28,
            "fixture: max(rgb) must span the measured spread; got {}",
            maxima.len()
        );

        let a8 = reduce_argb32_glyph_to_a8_coverage(&wire, 9, 9);
        assert_eq!(a8.len(), W * H);

        // The regression guard. Reading the alpha byte for coverage
        // yields 0xFF everywhere, i.e. a solid block — exactly the
        // reported defect, and invisible to any assertion weaker than
        // this one.
        assert!(
            a8.iter().any(|&v| v != a8[0]),
            "reduced coverage must vary across the glyph; a constant \
             plane means the alpha byte was read instead of R, G, B"
        );
        assert!(
            a8.iter().any(|&v| v != 0xFF),
            "reduced coverage must not be a solid 0xFF block"
        );

        // Exact spot checks, so the mapping is pinned and not merely
        // shown to be non-constant.
        assert_eq!(a8[0], 5, "(0,0): B=0x00 G=0x05 R=0x0B → 5");
        assert_eq!(a8[4 * W + 4], 160, "(4,4): B=0x90 G=0xA1 R=0xAF → 160");
        assert_eq!(a8[8 * W + 8], 59, "(8,8)");
    }

    #[test]
    fn a8_and_a1_glyphs_are_untouched_by_the_argb32_reduction() {
        // Invariant 1 of the design: an A8 or A1 glyphset must be
        // byte-identical before and after. That is the overwhelmingly
        // common path. Asserted here at the dispatch that gained the
        // new variant.
        let a8_src = [0x00u8, 0x7F, 0xFF, 0x40, 0xEE, 0xEE];
        let out = GlyphPixels::A8(&a8_src).to_a8(2, 2).expect("fits");
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "A8 must still borrow in place — no reduction, no copy"
        );
        assert_eq!(
            &*out,
            &a8_src[..4],
            "A8 coverage bytes must pass through unchanged"
        );

        let a1_wire = [0b0101_0011u8, 0, 0, 0];
        let out = GlyphPixels::A1Wire(&a1_wire).to_a8(8, 1).expect("fits");
        assert_eq!(
            &*out,
            &expand_a1_glyph_to_a8(&a1_wire, 8, 1)[..],
            "A1 must still take the LSBFirst expansion, unchanged"
        );
        assert_eq!(&*out, &[0xFF, 0xFF, 0, 0, 0xFF, 0, 0xFF, 0]);
    }

    // ── issue #77: bitmap fonts (terminus) rendered backwards ──
    // Guards the LSBFirst expansion order at its new home.

    #[test]
    fn a1_glyph_expands_lsb_first_not_mirrored() {
        // 8x1 glyph, one wire byte (padded to a 32-bit unit). Byte
        // 0b0000_0001 has only bit 0 set → LSBFirst means the LEFTMOST
        // pixel (col 0) is on and the rest are off.
        let wire = [0b0000_0001u8, 0, 0, 0];
        let a8 = expand_a1_glyph_to_a8(&wire, 8, 1);
        assert_eq!(
            a8,
            vec![0xFF, 0, 0, 0, 0, 0, 0, 0],
            "col 0 must be the set pixel (LSBFirst); an MSBFirst read \
             would light col 7 instead, mirroring the glyph"
        );

        // A left-to-right ramp: bits 0,1,2 set (cols 0,1,2), 0b0000_0111.
        let wire = [0b0000_0111u8, 0, 0, 0];
        let a8 = expand_a1_glyph_to_a8(&wire, 8, 1);
        assert_eq!(a8, vec![0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a1_glyph_expands_wide_glyph_across_scanline_units() {
        // 12px-wide glyph: still one 32-bit scanline unit (4 bytes) per
        // row, spanning two bytes of pixels. Verify column indexing is
        // continuous and LSBFirst across the byte boundary.
        // Byte 0 = 0b1000_0001 → cols 0 and 7 set.
        // Byte 1 = 0b0000_1000 → col 8+3 = 11 set.
        let wire = [0b1000_0001u8, 0b0000_1000u8, 0, 0];
        let a8 = expand_a1_glyph_to_a8(&wire, 12, 1);
        let mut expect = vec![0u8; 12];
        expect[0] = 0xFF;
        expect[7] = 0xFF;
        expect[11] = 0xFF;
        assert_eq!(a8, expect);
    }
}
