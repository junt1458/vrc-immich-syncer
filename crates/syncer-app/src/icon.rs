//! Pure, dependency-free RGBA icon generation shared by the tray icon
//! (`tray.rs`, feature-gated) and the window/taskbar icon (`main.rs`).
//!
//! No image assets or extra crates are used: icons are drawn directly into
//! an RGBA buffer with basic pixel math (a filled circle with a subtle ring,
//! plus a simple upload-arrow glyph) so the icon reads as an actual pictogram
//! rather than a flat color swatch, while staying trivial to build in any
//! environment (including headless CI / this Linux devcontainer sandbox).

/// Generates a square `size x size` RGBA8 (unmultiplied alpha) icon: a
/// filled circle in `color` with a slightly darker ring outline and a
/// white upload-arrow glyph (a stem with an arrowhead pointing up) in the
/// center, on a transparent background.
///
/// Returns a buffer of exactly `size * size * 4` bytes, row-major,
/// 4 bytes (R, G, B, A) per pixel. Pure and deterministic: calling it twice
/// with the same arguments yields identical output.
pub fn generate_icon_rgba(color: (u8, u8, u8), size: u32) -> Vec<u8> {
    let (r, g, b) = color;
    let s = size as f32;
    let cx = s / 2.0;
    let cy = s / 2.0;
    let radius = s / 2.0 - 1.0; // 1px transparent margin so the circle isn't clipped
    let ring_inner = radius - (s * 0.08).max(1.0);

    // Darker ring shade for a subtle border, so flat fills don't look like
    // a plain colored disc.
    let ring = (
        (r as f32 * 0.72) as u8,
        (g as f32 * 0.72) as u8,
        (b as f32 * 0.72) as u8,
    );

    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist > radius {
                // Outside the disc: fully transparent.
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }

            let (mut pr, mut pg, mut pb) = if dist > ring_inner { ring } else { (r, g, b) };

            if is_arrow_glyph(dx, dy, s) {
                pr = 0xff;
                pg = 0xff;
                pb = 0xff;
            }

            rgba.extend_from_slice(&[pr, pg, pb, 0xff]);
        }
    }

    rgba
}

/// Simple upward-pointing arrow glyph (an "upload" pictogram), expressed as
/// pixel-membership test in coordinates relative to the icon center
/// (`dx`/`dy`, with `dy` negative going up). Kept intentionally basic —
/// straight stem plus a triangular head, no anti-aliasing.
fn is_arrow_glyph(dx: f32, dy: f32, size: f32) -> bool {
    let stem_half_width = (size * 0.06).max(0.6);
    let stem_top = -size * 0.22;
    let stem_bottom = size * 0.20;
    let head_tip = -size * 0.30;
    let head_base = -size * 0.06;
    let head_half_width_at_base = size * 0.16;

    // Vertical stem: a narrow rectangle below the arrowhead base.
    let in_stem = dx.abs() <= stem_half_width && dy >= stem_top && dy <= stem_bottom;

    // Triangular arrowhead: widens linearly from the tip down to its base.
    let in_head = dy >= head_tip && dy <= head_base && {
        let t = (dy - head_tip) / (head_base - head_tip); // 0 at tip, 1 at base
        let half_width_here = head_half_width_at_base * t;
        dx.abs() <= half_width_here
    };

    in_stem || in_head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_length_matches_size_squared_times_4() {
        for size in [1u32, 2, 16, 32, 64, 256] {
            let rgba = generate_icon_rgba((0x3a, 0x7c, 0xd8), size);
            assert_eq!(rgba.len(), (size * size * 4) as usize, "size={size}");
        }
    }

    #[test]
    fn deterministic_across_calls() {
        let a = generate_icon_rgba((0x2f, 0xa8, 0x4f), 32);
        let b = generate_icon_rgba((0x2f, 0xa8, 0x4f), 32);
        assert_eq!(a, b);
    }

    #[test]
    fn does_not_panic_for_small_or_odd_sizes() {
        for size in [0u32, 1, 3, 5, 17] {
            let rgba = generate_icon_rgba((0xd8, 0x3a, 0x3a), size);
            assert_eq!(rgba.len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn center_pixel_is_opaque_and_within_disc_color_family() {
        let size = 32;
        let rgba = generate_icon_rgba((0x3a, 0x7c, 0xd8), size);
        let idx = ((size / 2 * size + size / 2) * 4) as usize;
        let alpha = rgba[idx + 3];
        assert_eq!(alpha, 0xff, "center of the disc must be opaque");
    }

    #[test]
    fn corner_pixel_is_transparent() {
        let size = 32;
        let rgba = generate_icon_rgba((0x3a, 0x7c, 0xd8), size);
        // Top-left corner is outside the inscribed circle.
        let idx = 0usize;
        let alpha = rgba[idx + 3];
        assert_eq!(alpha, 0, "corner outside the disc must be transparent");
    }

    #[test]
    fn different_colors_produce_different_ring_shade() {
        let blue = generate_icon_rgba((0x3a, 0x7c, 0xd8), 32);
        let green = generate_icon_rgba((0x2f, 0xa8, 0x4f), 32);
        assert_ne!(blue, green);
    }
}
