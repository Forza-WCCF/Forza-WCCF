//! Card sizes from the card pictures' own size. The HQ pack's scans are all 180x254 pixels (7,601 pictures, 99.99 %
//! within a pixel), so a card is drawn at a whole fraction of that in physical pixels (1, 1/2, 1/3: a clean downscale)
//! or, for the thumbnail grid only, 1.5x. Points convert with `pixels_per_point`, so Windows scaling (100/125/150/200 %)
//! changes the point size, never the sharpness.

use egui::{Vec2, vec2};

/// The pack's scan, in physical pixels.
pub const NATIVE: Vec2 = vec2(180.0, 254.0);
/// The grid's gap between cards, in points.
pub const GAP: f32 = 10.0;
/// The grid shows at least this many cards per row on a window wide enough for one.
const MIN_COLS: usize = 6;
/// Grid card widths as a part of the scan: 1.5x (the one upscale: a thumbnail), native, then clean downscales.
const GRID_STEPS: [f32; 4] = [1.5, 1.0, 0.5, 1.0 / 3.0];
/// The hover card's face is about this wide in points; its size is the whole-fraction of the scan nearest to it.
const HOVER_PTS: f32 = 100.0;

/// The catalogue grid: a card's size in points and how many sit in a row of `avail` points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub card: Vec2,
    pub cols: usize,
}

/// The largest card size (a step of [`GRID_STEPS`]) that still fits [`MIN_COLS`] in `avail` points; the smallest when
/// none does.
pub fn grid(avail: f32, ppp: f32) -> Grid {
    let cols = |card: Vec2| (((avail + GAP) / (card.x + GAP)).floor() as usize).max(1);
    let size = |k: f32| NATIVE * k / ppp;
    let step = GRID_STEPS
        .iter()
        .copied()
        .find(|&k| cols(size(k)) >= MIN_COLS)
        .unwrap_or(GRID_STEPS[GRID_STEPS.len() - 1]);
    let card = size(step);
    Grid {
        card,
        cols: cols(card),
    }
}

/// The hover card's face in points: the scan divided by a whole number (at least 1, so never upscaled), as near
/// [`HOVER_PTS`] wide as that gets.
pub fn hover_face(ppp: f32) -> Vec2 {
    let k = (NATIVE.x / (HOVER_PTS * ppp)).round().max(1.0);
    NATIVE / (k * ppp)
}

/// The most the board's big hover card enlarges a pitch card `tile_w` points wide: its scan at native pixels, between
/// 1.5x and 4x the tile (the board's `big_scale` takes the smaller of this and what the window leaves).
pub fn big_scale(tile_w: f32, ppp: f32) -> f32 {
    (NATIVE.x / ppp / tile_w).clamp(1.5, 4.0)
}

/// How much the hover card's text grows with its face: 1 at the base size (a 90 point face), within 0.9..1.3.
pub fn text_unit(face_w: f32) -> f32 {
    (face_w / 90.0).clamp(0.9, 1.3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue's results column in points for a window of `w` x `h` pixels at `dpi`: the sheet less the board,
    /// the stats pane and the page's margins (measured on the 1440x900 sheet and up: about 0.46 of the width).
    fn avail(w: f32, h: f32, dpi: f32) -> (f32, f32) {
        let ppp = crate::panel::scale(w, h, dpi);
        (w / ppp * 0.46, ppp)
    }

    #[test]
    fn the_grid_follows_the_pictures_own_size() {
        for (w, h, dpi, cols_min, cols_max) in [
            (1920.0, 1080.0, 96.0, 6, 9),  // 1080p at 100 %: half size, sharp
            (2560.0, 1440.0, 120.0, 6, 9), // 1440p at 125 %
            (3840.0, 2160.0, 96.0, 6, 9),  // 4K at 100 %: 12 before
            (3840.0, 2160.0, 144.0, 6, 9), // 4K at 150 %
        ] {
            let (a, ppp) = avail(w, h, dpi);
            let g = grid(a, ppp);
            assert!(
                (cols_min..=cols_max).contains(&g.cols),
                "{w}x{h}@{dpi}: {g:?}"
            );
            // whole physical pixels: a card of a clean fraction of the scan (or 1.5x)
            let px = g.card.x * ppp;
            assert!(
                [270.0, 180.0, 90.0, 60.0]
                    .iter()
                    .any(|n| (px - n).abs() < 0.01),
                "{px}"
            );
        }
    }

    #[test]
    fn the_hover_face_is_never_upscaled() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let f = hover_face(ppp);
            assert!(f.x * ppp <= NATIVE.x + 0.01, "{ppp}: {f:?}");
            assert!(f.x > 50.0 && f.x < 150.0, "{ppp}: {f:?}");
        }
        // a 58 point pitch card at 4K (2 points a pixel): the big card is the scan at native size, 90 points
        assert!((58.0 * big_scale(58.0, 2.0) * 2.0 - 180.0).abs() < 30.0);
        assert_eq!(hover_face(1.0).x, 90.0);
        assert_eq!(hover_face(2.0).x, 90.0);
    }
}
