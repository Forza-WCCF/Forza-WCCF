//! Card sizes in physical pixels. The catalogue grid fills its width: whole-pixel cards in the scan's shape (the HQ
//! pack's 180x254), as many a row as fit at most 170 points wide. The detail pane and the card view draw a picture at
//! a whole multiple or a whole fraction of its own pixels ([`whole_fit`], [`exact`]). Points convert with
//! `pixels_per_point`, so Windows scaling (100/125/150/200 %) changes the point size, never the sharpness.
use egui::{Vec2, vec2};

/// The pack's scan, in physical pixels.
pub const NATIVE: Vec2 = vec2(180.0, 254.0);
/// The grid's gap between cards, in points.
pub const GAP: f32 = 10.0;
/// A grid card is at most this wide, in points: about 6 a row at 1920x1080 beside the detail pane.
const MAX_CARD: f32 = 170.0;
/// The hover card's face is about this wide in points; its size is the whole-fraction of the scan nearest to it.
const HOVER_PTS: f32 = 100.0;

/// The catalogue grid: a card's size in points, how many sit in a row, and the margin left of the first one that
/// centres the rows in their width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub card: Vec2,
    pub cols: usize,
    pub pad: f32,
}

/// The grid that fills `avail` points: the fewest columns whose cards are at most [`MAX_CARD`] wide, each card as wide
/// as the row allows (whole physical pixels, the scan's shape), the pixels left over split on both sides.
pub fn grid(avail: f32, ppp: f32) -> Grid {
    let cols = (((avail + GAP) / (MAX_CARD + GAP)).ceil() as usize).max(1);
    let n = cols as f32;
    let w = ((avail - (n - 1.0) * GAP) / n * ppp).floor().max(1.0);
    let card = vec2(w, (w * NATIVE.y / NATIVE.x).round()) / ppp;
    let pad = ((avail - n * card.x - (n - 1.0) * GAP) / 2.0 * ppp)
        .floor()
        .max(0.0)
        / ppp;
    Grid { card, cols, pad }
}

/// The hover card's face in points: the scan divided by a whole number (at least 1, so never upscaled), as near
/// [`HOVER_PTS`] wide as that gets.
pub fn hover_face(ppp: f32) -> Vec2 {
    let k = (NATIVE.x / (HOVER_PTS * ppp)).round().max(1.0);
    NATIVE / (k * ppp)
}

/// The album art's front and back (`<number>_f.jpg`, `<number>_b.jpg`), in physical pixels: the slot the card view and
/// the large hover pane draw a card in, whatever picture it has.
pub const ALBUM: Vec2 = vec2(256.0, 357.0);
/// The card view's gap between its two pictures, in points.
pub const PAIR_GAP: f32 = 16.0;

/// The card view's scale: `n` pictures of `pic_px` pixels side by side ([`PAIR_GAP`] apart) drawn at a whole multiple
/// of their own pixels, 2 when that fits `room` (points, at `ppp` pixels a point), else 1; `None` when even 1 does not.
/// A picture is then `pic_px * k / ppp` points: never a fraction of its pixels, so never soft.
pub fn exact(pic_px: Vec2, n: usize, room: Vec2, ppp: f32) -> Option<f32> {
    let n = n as f32;
    [2.0, 1.0].into_iter().find(|k| {
        n * pic_px.x * k + (n - 1.0) * PAIR_GAP * ppp <= room.x * ppp + 0.01
            && pic_px.y * k <= room.y * ppp + 0.01
    })
}

/// The catalogue's detail pane width in points: room for the album art's 256 pixels and the pane's margins, at the
/// step [`whole_fit`] takes under the 330-point cap (x2 from about 167 %, else x1), within 250..330 points (under that,
/// [`whole_fit`] halves the picture).
pub fn detail_width(ppp: f32) -> f32 {
    // a point of slack beside the 512 pixels: the strip rounds its cell to the pixel
    let (k, slack) = if ALBUM.x * 2.0 / ppp + 25.0 <= 330.0 {
        (2.0, 1.0)
    } else {
        (1.0, 0.0)
    };
    (ALBUM.x * k / ppp + 24.0 + slack).clamp(250.0, 330.0)
}

/// The most the results header (the card count and the affinity row) may take of a cell `cell_h` points tall: the rest
/// is at least one row of `card_h` cards and the gap above them, however long the expanded affinity list is.
pub fn header_cap(cell_h: f32, card_h: f32) -> f32 {
    (cell_h - card_h - GAP).max(48.0)
}

/// A picture of `pic_px` pixels drawn whole in `room` points: its size in points at the largest of x2, x1, 1/2, 1/3 or
/// 1/4 of its pixels that fits (the smallest when none does). Every drawn pixel is then a whole number of the
/// picture's, or a whole number of them in one, so it is never soft at any window size or Windows scaling.
pub fn whole_fit(pic_px: Vec2, room: Vec2, ppp: f32) -> Vec2 {
    let steps = [2.0, 1.0, 0.5, 1.0 / 3.0, 0.25];
    let fits = |k: f32| pic_px.x * k <= room.x * ppp + 0.01 && pic_px.y * k <= room.y * ppp + 0.01;
    let k = steps.into_iter().find(|&k| fits(k)).unwrap_or(0.25);
    (pic_px * k).floor() / ppp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue's results column in points for a window of `w` x `h` pixels at `dpi`: the page beside the board,
    /// less its margins (12 around, 20 inside), the detail pane and the gap before it.
    fn avail(w: f32, h: f32, dpi: f32) -> (f32, f32) {
        let ppp = crate::panel::scale(w, h, dpi);
        let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(w, h) / ppp);
        let page = crate::panel::layout(sheet, false).pages.width() - 64.0;
        (page - detail_width(ppp) - 6.0, ppp)
    }

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn the_grid_fills_its_width() {
        for (w, h, dpi, cols_min) in [
            (1920.0, 1080.0, 96.0, 6),  // 1080p at 100 %
            (2014.0, 1080.0, 96.0, 6),  // the play-test window
            (2560.0, 1440.0, 120.0, 6), // 1440p at 125 %
            (3840.0, 2160.0, 144.0, 6), // 4K at 150 %
            (1440.0, 900.0, 96.0, 3),   // the base sheet
        ] {
            let (a, ppp) = avail(w, h, dpi);
            let g = grid(a, ppp);
            assert!(g.cols >= cols_min, "{w}x{h}@{dpi}: {g:?}");
            assert!(g.card.x <= MAX_CARD + 0.01, "{g:?}");
            // whole physical pixels, and the row less its two margins is the width, within a pixel a column
            let px = g.card * ppp;
            assert!(close(px, px.round()), "{px:?}");
            let row = 2.0 * g.pad + g.cols as f32 * g.card.x + (g.cols - 1) as f32 * GAP;
            assert!(
                row <= a + 0.01 && a - row < g.cols as f32 / ppp + 1.0 / ppp,
                "{a} {row} {g:?}"
            );
        }
    }

    /// The detail pane's picture: a whole multiple or a whole fraction of its pixels at every Windows scaling, the
    /// largest that fits.
    #[test]
    fn the_detail_picture_is_whole_pixels() {
        let room = vec2(276.0, 480.0);
        // x2 once the room holds 512 pixels; under 1 px a point (a window smaller than the sheet) half size
        for (ppp, k) in [
            (1.0, 1.0),
            (1.25, 1.0),
            (1.5, 1.0),
            (2.0, 2.0),
            (0.889, 0.5),
            (0.625, 0.5),
        ] {
            let s = whole_fit(ALBUM, room, ppp);
            assert!(close(s * ppp, (ALBUM * k).floor()), "{ppp}: {s:?}");
        }
        // the pack's 180x254 scan doubles when the room takes it, the 664x911 picture is a third
        assert_eq!(whole_fit(NATIVE, vec2(400.0, 600.0), 1.0), NATIVE * 2.0);
        assert_eq!(whole_fit(vec2(664.0, 911.0), room, 1.0), vec2(221.0, 303.0));
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let px = whole_fit(vec2(664.0, 911.0), room, ppp) * ppp;
            assert!(close(px, px.round()), "{ppp}: {px:?}");
            assert!(
                px.x / ppp <= room.x + 0.01 && px.y / ppp <= room.y + 0.01,
                "{ppp}"
            );
        }
    }

    /// At 175 and 200 % the pane is sized for the picture's x2 step, so the detail picture is wider than a grid card
    /// (sized at x1 it stayed 256 pixels against a 267- and a 323-pixel card).
    #[test]
    fn the_detail_picture_is_never_smaller_than_a_card_at_high_scaling() {
        for (w, h, dpi) in [(3840.0, 2160.0, 168.0), (3840.0, 2160.0, 192.0)] {
            let (a, ppp) = avail(w, h, dpi);
            assert!([1.75, 2.0].contains(&ppp), "{w}x{h}@{dpi}: {ppp}");
            let card = grid(a, ppp).card.x * ppp;
            let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(w, h) / ppp);
            let page = crate::panel::layout(sheet, false).pages;
            // the pane's inner width less its frame, and the height its picture has under the rest of the pane
            let room = vec2(detail_width(ppp) - 24.0, page.height() - 300.0);
            let picture = whole_fit(ALBUM, room, ppp) * ppp;
            assert!(close(picture, ALBUM * 2.0), "{ppp} px a point: {picture:?}");
            assert!(
                picture.x >= card,
                "{ppp} px a point: a {} px picture against a {card} px card ({room:?})",
                picture.x
            );
        }
    }

    /// The header's cap always leaves the grid a row of cards and the gap above it.
    #[test]
    fn the_catalogue_header_leaves_a_row_of_cards() {
        for cell in [300.0, 650.0, 900.0] {
            for card in [20.0, 120.0, 170.0] {
                let cap = header_cap(cell, card);
                assert!(cap >= 48.0, "{cell} {card}: {cap}");
                assert!(cell - cap >= card + GAP, "{cell} {card}: {cap}");
            }
        }
    }

    /// The card view's scale is a whole multiple of the pictures' pixels, the largest of 1 and 2 that fits: the pair at
    /// x1 on the 1440x900 sheet, one face at x1 on a 900x563 window (0.625 px a point), the pair at x2 at 175 %.
    #[test]
    fn the_card_view_draws_whole_pixels() {
        // the catalogue page's body on the 1440x900 sheet (about 804x730 points), less the head row and the caption
        let room = vec2(804.0, 730.0 - 48.0 - 100.0);
        for ppp in [0.625, 0.889, 1.0, 1.25, 1.5, 1.75, 2.0] {
            for n in [1, 2] {
                if let Some(k) = exact(ALBUM, n, room, ppp) {
                    assert!(k == 1.0 || k == 2.0, "{ppp} {n}: {k}");
                    let size = ALBUM * k / ppp;
                    assert_eq!(size * ppp / k, ALBUM, "{ppp} {n}");
                    let w = n as f32 * size.x + (n as f32 - 1.0) * PAIR_GAP;
                    assert!(
                        w <= room.x + 0.01 && size.y <= room.y + 0.01,
                        "{ppp} {n}: {size:?}"
                    );
                }
            }
        }
        assert_eq!(exact(ALBUM, 2, room, 1.0), Some(1.0));
        assert_eq!(exact(ALBUM, 2, room, 0.625), None);
        assert_eq!(exact(ALBUM, 1, room, 0.625), Some(1.0));
        assert_eq!(exact(ALBUM, 2, room, 1.75), Some(2.0));
    }

    #[test]
    fn the_hover_face_is_never_upscaled() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let f = hover_face(ppp);
            assert!(f.x * ppp <= NATIVE.x + 0.01, "{ppp}: {f:?}");
            assert!(f.x > 50.0 && f.x < 150.0, "{ppp}: {f:?}");
        }
        assert_eq!(hover_face(1.0).x, 90.0);
        assert_eq!(hover_face(2.0).x, 90.0);
    }
}
