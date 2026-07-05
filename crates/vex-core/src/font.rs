//! Original angular stroke font in the vector-arcade tradition: every glyph
//! is polylines on a 4×6 grid, rendered by the same line pass as the world.
//! Zero-length segments become round dots via the renderer's capsule caps
//! (that is how `.` `:` `%` get their marks).
//!
//! Coverage: A–Z, 0–9, and a handful of HUD punctuation. Unknown characters
//! render as an empty advance.

use glam::{Vec2, Vec4, vec3};

use crate::Segment;

/// Glyph grid height; text scale maps this to the requested pixel height.
const GRID_HEIGHT: f32 = 6.0;
/// Horizontal advance in grid units (4-wide glyphs + 1.5 spacing).
const ADVANCE: f32 = 5.5;

type Strokes = &'static [&'static [(i8, i8)]];

#[rustfmt::skip]
fn strokes(c: char) -> Strokes {
    match c {
        'A' => &[&[(0,0),(0,4),(2,6),(4,4),(4,0)], &[(0,3),(4,3)]],
        'B' => &[&[(0,0),(0,6),(3,6),(4,5),(4,4),(3,3),(0,3)], &[(3,3),(4,2),(4,1),(3,0),(0,0)]],
        'C' => &[&[(4,1),(3,0),(1,0),(0,1),(0,5),(1,6),(3,6),(4,5)]],
        'D' => &[&[(0,0),(0,6),(2,6),(4,4),(4,2),(2,0),(0,0)]],
        'E' => &[&[(4,0),(0,0),(0,6),(4,6)], &[(0,3),(3,3)]],
        'F' => &[&[(0,0),(0,6),(4,6)], &[(0,3),(3,3)]],
        'G' => &[&[(4,5),(3,6),(1,6),(0,5),(0,1),(1,0),(3,0),(4,1),(4,3),(2,3)]],
        'H' => &[&[(0,0),(0,6)], &[(4,0),(4,6)], &[(0,3),(4,3)]],
        'I' => &[&[(1,0),(3,0)], &[(2,0),(2,6)], &[(1,6),(3,6)]],
        'J' => &[&[(0,1),(1,0),(3,0),(4,1),(4,6)]],
        'K' => &[&[(0,0),(0,6)], &[(4,6),(0,3),(4,0)]],
        'L' => &[&[(0,6),(0,0),(4,0)]],
        'M' => &[&[(0,0),(0,6),(2,3),(4,6),(4,0)]],
        'N' => &[&[(0,0),(0,6),(4,0),(4,6)]],
        'O' => &[&[(1,0),(0,1),(0,5),(1,6),(3,6),(4,5),(4,1),(3,0),(1,0)]],
        'P' => &[&[(0,0),(0,6),(3,6),(4,5),(4,4),(3,3),(0,3)]],
        'Q' => &[&[(1,0),(0,1),(0,5),(1,6),(3,6),(4,5),(4,1),(3,0),(1,0)], &[(2,2),(4,0)]],
        'R' => &[&[(0,0),(0,6),(3,6),(4,5),(4,4),(3,3),(0,3)], &[(2,3),(4,0)]],
        'S' => &[&[(4,5),(3,6),(1,6),(0,5),(0,4),(4,2),(4,1),(3,0),(1,0),(0,1)]],
        'T' => &[&[(0,6),(4,6)], &[(2,6),(2,0)]],
        'U' => &[&[(0,6),(0,1),(1,0),(3,0),(4,1),(4,6)]],
        'V' => &[&[(0,6),(2,0),(4,6)]],
        'W' => &[&[(0,6),(1,0),(2,3),(3,0),(4,6)]],
        'X' => &[&[(0,0),(4,6)], &[(0,6),(4,0)]],
        'Y' => &[&[(0,6),(2,3),(4,6)], &[(2,3),(2,0)]],
        'Z' => &[&[(0,6),(4,6),(0,0),(4,0)]],
        '0' => &[&[(1,0),(0,1),(0,5),(1,6),(3,6),(4,5),(4,1),(3,0),(1,0)], &[(1,1),(3,5)]],
        '1' => &[&[(1,5),(2,6),(2,0)], &[(1,0),(3,0)]],
        '2' => &[&[(0,5),(1,6),(3,6),(4,5),(4,4),(0,0),(4,0)]],
        '3' => &[&[(0,5),(1,6),(3,6),(4,5),(4,4),(3,3),(4,2),(4,1),(3,0),(1,0),(0,1)], &[(2,3),(3,3)]],
        '4' => &[&[(3,0),(3,6),(0,2),(4,2)]],
        '5' => &[&[(4,6),(0,6),(0,4),(3,4),(4,3),(4,1),(3,0),(1,0),(0,1)]],
        '6' => &[&[(3,6),(1,6),(0,5),(0,1),(1,0),(3,0),(4,1),(4,2),(3,3),(0,3)]],
        '7' => &[&[(0,6),(4,6),(1,0)]],
        '8' => &[&[(1,3),(0,4),(0,5),(1,6),(3,6),(4,5),(4,4),(3,3),(4,2),(4,1),(3,0),(1,0),(0,1),(0,2),(1,3)], &[(1,3),(3,3)]],
        '9' => &[&[(1,0),(3,0),(4,1),(4,5),(3,6),(1,6),(0,5),(0,4),(1,3),(4,3)]],
        '-' => &[&[(1,3),(3,3)]],
        '+' => &[&[(0,3),(4,3)], &[(2,1),(2,5)]],
        '.' => &[&[(2,0),(2,0)]],
        ':' => &[&[(2,1),(2,1)], &[(2,5),(2,5)]],
        '/' => &[&[(0,0),(4,6)]],
        '%' => &[&[(0,0),(4,6)], &[(1,5),(1,5)], &[(3,1),(3,1)]],
        _ => &[],
    }
}

/// Lay out `text` as stroke segments in screen space (x right, y up),
/// baseline starting at `origin` (pixels), glyphs `px_height` tall.
/// Lowercase is rendered as uppercase.
pub fn text_segments(text: &str, origin: Vec2, px_height: f32, color: Vec4) -> Vec<Segment> {
    let scale = px_height / GRID_HEIGHT;
    let mut segments = Vec::new();
    let mut pen_x = origin.x;
    for c in text.chars() {
        for polyline in strokes(c.to_ascii_uppercase()) {
            for pair in polyline.windows(2) {
                let (x0, y0) = pair[0];
                let (x1, y1) = pair[1];
                segments.push(Segment::new(
                    vec3(pen_x + f32::from(x0) * scale, origin.y + f32::from(y0) * scale, 0.0),
                    vec3(pen_x + f32::from(x1) * scale, origin.y + f32::from(y1) * scale, 0.0),
                    color,
                ));
            }
            // Single-point polylines are dots: emit a zero-length segment.
            if polyline.len() == 1 {
                let (x, y) = polyline[0];
                let p = vec3(pen_x + f32::from(x) * scale, origin.y + f32::from(y) * scale, 0.0);
                segments.push(Segment::new(p, p, color));
            }
        }
        pen_x += ADVANCE * scale;
    }
    segments
}

/// Width in pixels that `text` will occupy at `px_height`.
pub fn text_width(text: &str, px_height: f32) -> f32 {
    text.chars().count() as f32 * ADVANCE * (px_height / GRID_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec4;

    #[test]
    fn text_produces_strokes_and_advances() {
        let color = vec4(1.0, 0.0, 0.0, 1.0);
        let segments = text_segments("HI", Vec2::ZERO, 12.0, color);
        assert!(!segments.is_empty());
        // H = 3 polylines (2+2+2 pts) → 3 segments; I = 3 polylines → 3.
        assert_eq!(segments.len(), 6);
        // Second glyph starts one advance to the right.
        let advance_px = ADVANCE * 12.0 / GRID_HEIGHT;
        assert!(segments[3].a.x >= advance_px - 1e-4);
    }

    #[test]
    fn dots_are_zero_length_segments() {
        let segments = text_segments(".", Vec2::ZERO, 12.0, Vec4::ONE);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].a, segments[0].b);
    }

    #[test]
    fn unknown_chars_advance_silently() {
        let segments = text_segments("A#B", Vec2::ZERO, 12.0, Vec4::ONE);
        let only_ab = text_segments("AB", Vec2::ZERO, 12.0, Vec4::ONE);
        assert_eq!(segments.len(), only_ab.len());
    }

    #[test]
    fn width_matches_advance() {
        assert!((text_width("100", 18.0) - 3.0 * ADVANCE * 3.0).abs() < 1e-4);
    }
}
