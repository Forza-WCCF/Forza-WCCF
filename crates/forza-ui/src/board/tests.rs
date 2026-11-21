use super::*;

/// The 5 % gate: the pack's 180x254 scan and the album's 256x357 art fill each other's slot; the game's square
/// picture, and a shape just past 5 % either way, are letterboxed whole inside it.
#[test]
fn a_card_shaped_picture_fills_its_slot_and_another_is_letterboxed() {
    let slot = Rect::from_min_size(pos2(10.0, 20.0), vec2(256.0, 357.0));
    let k = slot.aspect_ratio();
    for aspect in [180.0 / 254.0, k, k * 1.049, k * 0.951] {
        assert_eq!(picture_rect(slot, aspect), slot, "{aspect}: fills the slot");
    }
    for aspect in [1.0, k * 1.051, k * 0.949] {
        let at = picture_rect(slot, aspect);
        assert!(at != slot && slot.contains_rect(at), "{aspect}: {at:?}");
        assert!(
            (at.aspect_ratio() - aspect).abs() < 1e-4,
            "{aspect}: its own shape"
        );
        assert_eq!(at.center(), slot.center(), "{aspect}: centred");
    }
}

#[test]
fn a_rating_comes_from_the_area_drawn_on_top() {
    let mut a = Aptitude::default();
    a.roles[0] = 9; // CF
    a.roles[3] = 4; // SS
    a.roles[11] = 8; // CB
    a.roles[14] = 6; // SW: CB's area, the better role counts
    assert_eq!(a.at(pos2(0.5, 0.03)), 9.0); // CF's top strip
    assert_eq!(a.at(pos2(0.5, 0.2)), 4.0); // SS inside CF
    assert_eq!(a.at(pos2(0.5, 0.8)), 8.0);
    assert_eq!(a.at(pos2(0.5, 0.95)), 0.0); // the keeper's box inside CB's
    assert_eq!(a.at(pos2(0.1, 0.1)), 0.0); // WG
}

#[test]
fn a_card_dropped_on_the_next_one_targets_it() {
    // slot 0 follows the pointer and overlaps slot 1: the target is slot 1, never the dragged slot itself
    let mut rects = [Rect::NOTHING; 16];
    rects[0] = Rect::from_min_size(pos2(90.0, 0.0), vec2(80.0, 120.0));
    rects[1] = Rect::from_min_size(pos2(100.0, 0.0), vec2(80.0, 120.0));
    assert_eq!(
        target_slot(&rects, |_| true, Some(0), pos2(150.0, 50.0)),
        Some(1)
    );
    assert_eq!(
        target_slot(&rects, |_| true, Some(0), pos2(95.0, 50.0)),
        None
    );
}

#[test]
fn the_top_drawn_of_two_overlapping_cards_is_the_target() {
    let mut rects = [Rect::NOTHING; 16];
    rects[3] = Rect::from_min_size(pos2(0.0, 0.0), vec2(80.0, 120.0));
    rects[7] = Rect::from_min_size(pos2(40.0, 0.0), vec2(80.0, 120.0));
    assert_eq!(
        target_slot(&rects, |_| true, None, pos2(60.0, 50.0)),
        Some(7)
    );
}

#[test]
fn a_dragged_card_stays_where_it_was_taken() {
    // taken at its bottom-right corner: the first frame draws it where it was, and it follows the pointer
    let card = vec2(80.0, 120.0);
    let rest = pos2(300.0, 200.0);
    let press = rest + card / 2.0;
    let grab = (rest - press) / card;
    assert_eq!(drag_centre(press, grab, card), rest);
    let moved = press + vec2(10.0, -5.0);
    assert_eq!(drag_centre(moved, grab, card), rest + vec2(10.0, -5.0));
}

#[test]
fn a_card_picture_is_shown_whole() {
    // the game's whole 128x128 picture: its face, the left 88 columns, all its height
    let (uv, a) = face_uv(vec2(128.0, 128.0));
    assert_eq!(
        (uv.min, uv.max, a),
        (pos2(0.0, 0.0), pos2(0.6875, 1.0), 0.6875)
    );
    // the pack's 180x254 scan: all of it, and the tile has its shape
    let (uv, a) = face_uv(vec2(180.0, 254.0));
    assert_eq!(
        (uv.min, uv.max, a),
        (pos2(0.0, 0.0), pos2(1.0, 1.0), CARD_ASPECT)
    );
    // the album's 256x357 front: all of it too, about 1 % wider than the tile (thin bars above and below, no stretch)
    let (uv, a) = face_uv(vec2(256.0, 357.0));
    assert_eq!((uv.min, uv.max), (pos2(0.0, 0.0), pos2(1.0, 1.0)));
    assert!((a / CARD_ASPECT - 1.0).abs() < 0.02, "{a}");
    // the downloaded picture first, then the album art, the older pack's scan, the game's face
    assert_eq!(
        picture_names(7049),
        ["7049_hq.jpg", "7049_f.jpg", "7049.jpg", "7049.png"]
    );
    let tile = card_size(400.0);
    assert!((tile.x / tile.y - CARD_ASPECT).abs() < 0.01, "{tile:?}");
    // the game's narrower face in a card tile: full height, bars at the sides, nothing cut
    let r = Rect::from_min_size(pos2(10.0, 20.0), tile);
    let l = letterbox(r, 0.6875);
    assert!(r.contains_rect(l) && (l.height() - r.height()).abs() < 1e-3);
    assert!((l.width() / l.height() - 0.6875).abs() < 1e-4 && l.center() == r.center());
    // a wider picture: full width, bars above and below
    let l = letterbox(r, 1.0);
    assert!((l.width() - r.width()).abs() < 1e-3 && l.height() < r.height());
}

/// A card with no back picture (no picture folder) has none (no BACK offered), its back side draws its front (the
/// drawn back is gone, #333), and the info pane's caption carries the back's facts: number, position, TOTAL.
#[test]
fn a_card_without_a_back_picture_draws_its_front_and_the_caption_has_its_facts() {
    let c = Card {
        name: "Andrea Pirlo".into(),
        club: "AC Milan".into(),
        season: "2006-07".into(),
        stats: [11, 12, 13, 14, 15, 16],
        total: 81,
        ..card(4321)
    };
    let ctx = egui::Context::default();
    crate::theme::install(
        &ctx,
        crate::theme::Edition::E1718,
        &crate::theme::font_dir(),
    )
    .unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let texts = |back: Option<bool>| {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            set_picture_dir(ui.ctx(), None);
            assert_eq!(has_back(ui.ctx(), c.number), Some(false));
            match back {
                Some(back) => {
                    let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(256.0, 357.0));
                    card_side(ui.painter(), r, &c, back, false);
                }
                None => caption(ui, &c, 1.0, crate::Lang::En),
            }
        });
        out.textures_delta.clear();
        let mut texts = Vec::new();
        for s in out.shapes {
            if let egui::Shape::Text(t) = s.shape {
                texts.push(t.galley.text().to_owned());
            }
        }
        texts
    };
    let front = texts(Some(false));
    assert!(!front.is_empty());
    assert_eq!(texts(Some(true)), front, "the back side draws the front");
    let cap = texts(None);
    for want in ["Andrea Pirlo", "#4321 · MF · TOTAL 81", "OFF", "11", "16"] {
        assert!(cap.iter().any(|t| t == want), "{want} not in {cap:?}");
    }
}

fn card(n: u32) -> Card {
    Card {
        number: n,
        record: None,
        name: n.to_string(),
        short: String::new(),
        club: String::new(),
        country: String::new(),
        season: String::new(),
        born: String::new(),
        pos: Position::Mf,
        rarity: Rarity::Regular,
        code: String::new(),
        secret: false,
        stats: [0; 6],
        total: 0,
        aptitude: Aptitude::default(),
    }
}

/// Cards 1..=3 on pitch slots 0..=2, card 4 on the first bench box.
fn table() -> Board {
    let mut b = Board {
        cards: (1..=4).map(card).collect(),
        slots: [None; 16],
        spots: [pos2(0.5, 0.5); 11],
        rub: None,
    };
    b.slots[..3].copy_from_slice(&[Some(0), Some(1), Some(2)]);
    b.slots[11] = Some(3);
    b
}

#[test]
fn a_card_moves_swaps_and_goes_to_the_bench() {
    let mut b = table();
    // free move on the pitch: the spot changes, the slot stays
    assert!(b.move_to(1, pos2(0.1, 0.2)));
    assert_eq!((b.spots[1], b.number(1)), (pos2(0.1, 0.2), Some(2)));
    // swap with another card, a bench card, and an empty bench box
    b.swap(0, 2);
    assert_eq!((b.number(0), b.number(2)), (Some(3), Some(1)));
    b.swap(0, 11);
    assert_eq!((b.number(0), b.number(11)), (Some(4), Some(3)));
    b.swap(1, 13);
    assert_eq!((b.number(1), b.number(13)), (None, Some(2)));
    // a bench card dropped on the pitch takes the first free pitch slot, at the drop spot
    assert!(b.move_to(13, pos2(0.9, 0.9)));
    assert_eq!((b.number(1), b.spots[1]), (Some(2), pos2(0.9, 0.9)));
    assert_eq!(b.number(13), None);
}

#[test]
fn a_catalogue_card_goes_on_the_pitch_or_over_a_place() {
    let mut b = table();
    assert!(b.put_free(&card(9), pos2(0.3, 0.3)));
    assert_eq!((b.number(3), b.spots[3]), (Some(9), pos2(0.3, 0.3)));
    // already on the table: refused
    assert!(!b.put_free(&card(9), pos2(0.0, 0.0)) && !b.put(&card(1), 5));
    // over a card: it takes the place and the old card goes to the first free bench box
    assert!(b.put(&card(10), 0));
    assert_eq!((b.number(0), b.number(12)), (Some(10), Some(1)));
    // a full bench: the old card goes off
    for slot in &mut b.slots[11..16] {
        slot.get_or_insert(0);
    }
    assert!(b.put(&card(11), 2));
    assert!(!b.holds(3) && b.number(2) == Some(11));
}

#[test]
fn one_card_per_player_on_the_table() {
    // card 2 is Thiago Silva 2013-14 on pitch slot 1; 20 and 21 are his other cards, 22 a namesake born elsewhen
    let silva = |n: u32, born: &str, name: &str| Card {
        name: name.into(),
        born: born.into(),
        ..card(n)
    };
    let mut b = table();
    b.cards[1] = silva(2, "1984-09-22", "THIAGO SILVA");
    // dropped on another place: the old card leaves the table, it does not go to the bench
    assert!(b.put(&silva(20, "1984-09-22", "Thiago SILVA"), 5));
    assert!(!b.holds(2) && b.number(5) == Some(20) && b.number(1).is_none());
    // dropped on open pitch: the same
    assert!(b.put_free(&silva(21, "1984-09-22", "THIAGO SÍLVA"), pos2(0.4, 0.4)));
    assert!(!b.holds(20) && b.holds(21));
    // the same name, another player
    assert!(b.put_free(&silva(22, "1990-01-01", "THIAGO SILVA"), pos2(0.6, 0.6)));
    assert!(b.holds(21) && b.holds(22));
}

#[test]
fn what_the_board_took_in_is_written_to_the_table() {
    use forza::cards::{Placed, table_after};
    let before: Vec<Placed> = (1..=3)
        .map(|i| Placed {
            slot: i,
            no: 100 + i as u32,
            x: -0.5 + i as f32 / 4.0,
            y: 0.1,
        })
        .collect();
    let mut b = forza::cards::board(&before, &[]);
    assert!(b.put_free(&card(555), pos2(0.5, 0.75)));
    let after = table_after(&before, &b);
    assert_eq!(after[..3], before[..]);
    assert_eq!((after[3].slot, after[3].no), (4, 555));
    assert_eq!((after[3].x, after[3].y), (0.0, 0.5));
}

/// Free placement through to the table file: a card dropped anywhere on the pitch keeps that exact place (three
/// decimals), a drop past the pitch is clamped to its lines, a bench card and a catalogue card land where dropped,
/// a drop onto a card swaps the two, and a card dropped on a bench box takes the box's band.
#[test]
fn cards_lie_where_they_are_dropped() {
    use forza::cards::{Placed, table_after, table_text};
    let pitch = pitch_rect(pos2(100.0, 50.0), 400.0);
    let f = field(pitch);
    let at = |x: f32, y: f32| pos2(f.left() + x * f.width(), f.top() + y * f.height());
    let pl = |slot, no, x, y| Placed { slot, no, x, y };
    let before = vec![
        pl(1, 101, 0.0, 0.9),
        pl(2, 102, -0.6, 0.55),
        pl(3, 103, 1.24, 0.34),
    ];
    let mut b = forza::cards::board(&before, &[]);
    // anywhere on the pitch: exactly there
    assert!(b.move_to(0, spot_at(pitch, at(0.3215, 0.8))));
    // past the pitch: clamped to the touchline and the far goal line
    assert!(b.move_to(1, spot_at(pitch, pitch.right_top() + vec2(90.0, -90.0))));
    let t = table_after(&before, &b);
    assert_eq!(
        t,
        [pl(1, 101, -0.357, 0.6), pl(2, 102, 1.0, -1.0), before[2]]
    );
    // the bench card (box 14, band 0.34) onto open pitch: there, not on a 4-4-2 spot
    assert_eq!(b.slots[14], Some(2));
    assert!(b.move_to(14, spot_at(pitch, at(0.75, 0.25))));
    // a catalogue card dropped on open pitch: there too
    assert!(b.put_free(&card(555), spot_at(pitch, at(0.1, 0.5))));
    let t = table_after(&before, &b);
    assert_eq!(t[2], pl(3, 103, 0.5, -0.5));
    assert_eq!(t[3], pl(4, 555, -0.8, 0.0));
    assert!(table_text(&t).ends_with("3 103 0.500 -0.500\n4 555 -0.800 0.000\n"));
    // onto another card: the two trade places
    let (s103, s555) = (b.slots.iter().position(|&s| s == Some(2)).unwrap(), 3);
    b.swap(s103, s555);
    let t2 = table_after(&t, &b);
    assert_eq!((t2[2].x, t2[2].y, t2[3].x, t2[3].y), (-0.8, 0.0, 0.5, -0.5));
    // onto the last bench box: that box's band
    b.swap(0, 15);
    let t3 = table_after(&t2, &b);
    assert_eq!((t3[0].x, t3[0].y), (forza::cards::BENCH_X, 0.68));
}

/// A pitch card in hand moves the table while the button is still down, back and forth (a rub), then once more on
/// release; the board keeps its slots throughout.
#[test]
fn a_card_in_hand_moves_the_table_before_release() {
    use egui::{Event, PointerButton};
    use forza::cards::{Placed, table_after};
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: 0.0,
        y: 0.0,
    }];
    let mut b = forza::cards::board(&before, &[]);
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    // the fonts take effect from the next frame
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut pitch = Rect::NOTHING;
    let frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut changed = false;
        ctx.run_ui(raw, |ui| {
            *pitch = pitch_rect(ui.cursor().min, ui.available_width());
            changed = formation_board(
                ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                false,
                false,
            );
        })
        .textures_delta
        .clear();
        changed
    };
    frame(vec![], &mut b, &mut pitch);
    let at = spot_rect(pitch, b.spots[0]).center();
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(
        vec![Event::PointerMoved(at), press(at, true)],
        &mut b,
        &mut pitch,
    );
    let mut xs = Vec::new();
    for dx in [10.0, 20.0, 40.0, 60.0, 20.0, 60.0, 20.0] {
        let changed = frame(
            vec![Event::PointerMoved(at + vec2(dx, 0.0))],
            &mut b,
            &mut pitch,
        );
        if changed {
            xs.push(table_after(&before, &b)[0].x);
        }
    }
    assert_eq!(b.slots[0], Some(0));
    // the held card moved right, back, right, back: four turns the game can see
    let turns = xs
        .windows(3)
        .filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0)
        .count();
    assert!(xs.len() >= 4 && turns >= 2, "{xs:?}");
    // let go: one more change, where it was shown
    assert!(frame(
        vec![press(at + vec2(20.0, 0.0), false)],
        &mut b,
        &mut pitch
    ));
    assert_eq!(table_after(&before, &b)[0].x, *xs.last().unwrap());
}

/// UNDO puts a card taken off back where it lay: a pitch card on its spot, a bench card in its box, after the
/// table was rebuilt as the panel DLL rebuilds it.
#[test]
fn undo_puts_a_removed_card_back_in_its_place() {
    use forza::cards::{BENCH_X, BENCH_Y, Placed, board as rebuild, table_after};
    let before = vec![
        Placed {
            slot: 1,
            no: 101,
            x: -0.4,
            y: 0.2,
        },
        Placed {
            slot: 2,
            no: 102,
            x: BENCH_X,
            y: BENCH_Y[3],
        },
        Placed {
            slot: 3,
            no: 103,
            x: 0.4,
            y: 0.6,
        },
    ];
    let cat: Vec<Card> = [101, 102, 103].map(card).into();
    let b0 = rebuild(&before, &cat);
    for slot in (0..16).filter(|&s| b0.slots[s].is_some()) {
        let mut b = b0.clone();
        let ci = b.slots[slot].take().unwrap();
        let r = Removed {
            card: b.cards[ci].clone(),
            slot,
            spot: b.spots.get(slot).copied().unwrap_or_default(),
            at: Pos2::ZERO,
            size: Vec2::ZERO,
        };
        let gone = table_after(&before, &b);
        assert_eq!(gone.len(), 2);
        let mut b = rebuild(&gone, &cat);
        assert!(undo(&mut b, &r));
        assert!(!undo(&mut b, &r), "on the table already");
        let back = table_after(&gone, &b);
        let was = before.iter().find(|p| p.no == r.card.number).unwrap();
        let now = back.iter().find(|p| p.no == r.card.number).unwrap();
        assert_eq!((now.x, now.y), (was.x, was.y), "slot {slot}");
        assert_eq!(rebuild(&back, &cat).slots.iter().flatten().count(), 3);
    }
}

/// The pop-out board: the pitch and the bench column side by side with no gap, the boxes in the column, every pitch
/// card on the pitch as shown; a pitch card dragged onto a bench box goes there, and back onto open pitch.
#[test]
fn the_column_board_benches_and_fields_a_card() {
    use egui::{Event, PointerButton};
    use forza::cards::Placed;
    let m = Mat::column(pos2(10.0, 20.0), 600.0);
    assert_eq!(m.shown.right(), m.bench.left());
    assert_eq!(
        (m.shown.top(), m.shown.bottom()),
        (m.bench.top(), m.bench.bottom())
    );
    assert!((m.shown.union(m.bench).height() - 600.0 * COLUMN_ASPECT).abs() < 0.01);
    assert!((m.bench.right() - 610.0).abs() < 0.01);
    for i in 0..5 {
        assert!(m.bench.contains_rect(m.bench_box(i)), "box {i}");
    }
    let corners = [
        pos2(0.0, 0.0),
        pos2(1.0, 0.0),
        pos2(0.0, 1.0),
        pos2(1.0, 1.0),
    ];
    let rects = m.slot_rects(&[
        corners[0],
        corners[1],
        corners[2],
        corners[3],
        pos2(0.5, 0.5),
        pos2(0.5, 0.95),
        pos2(0.5, 0.5),
        pos2(0.5, 0.5),
        pos2(0.5, 0.5),
        pos2(0.5, 0.5),
        pos2(0.5, 0.5),
    ]);
    for (i, r) in rects.iter().enumerate() {
        let within = if i < 11 { m.shown } else { m.bench };
        assert!(within.contains_rect(*r), "slot {i}: {r:?}");
    }

    let mut b = forza::cards::board(
        &[Placed {
            slot: 1,
            no: 101,
            x: 0.0,
            y: 0.0,
        }],
        &[],
    );
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut mat = None;
    let mut frame = |events: Vec<Event>, b: &mut Board| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(620.0, 900.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            mat = Some(Mat::column(ui.cursor().min, ui.available_width()));
            formation_board(
                ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                true,
                false,
            );
        })
        .textures_delta
        .clear();
        mat.unwrap()
    };
    let m = frame(vec![], &mut b);
    let mut drag = |from: Pos2, to: Pos2, b: &mut Board| {
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![Event::PointerMoved(from), press(from, true)], b);
        for p in [from.lerp(to, 0.1), from.lerp(to, 0.5), to] {
            frame(vec![Event::PointerMoved(p)], b);
        }
        frame(vec![press(to, false)], b);
        frame(vec![], b);
    };
    let rects = m.slot_rects(&b.spots);
    drag(rects[0].center(), m.bench_box(2).center(), &mut b);
    assert_eq!(b.slots[13], Some(0), "benched in box 3: {:?}", b.slots);
    let to = m.shown.center();
    drag(m.slot_rects(&b.spots)[13].center(), to, &mut b);
    let on = (0..11).find(|&s| b.slots[s] == Some(0));
    assert!(on.is_some(), "back on the pitch: {:?}", b.slots);
    let d = m.slot_rects(&b.spots)[on.unwrap()].center() - to;
    assert!(d.length() < 2.0, "where dropped: {d:?}");
}

/// The pop-out board (`pure`): a press reports the card touched, a resting mouse or a click never shows it big nor
/// selects it.
#[test]
fn a_pure_board_reports_the_touch_and_shows_nothing() {
    use egui::{Event, PointerButton};
    use forza::cards::Placed;
    let mut b = forza::cards::board(
        &[Placed {
            slot: 1,
            no: 101,
            x: 0.0,
            y: 0.0,
        }],
        &[],
    );
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    let links = Links {
        club: [((101, 102), 200)].into_iter().collect(),
        affinity: None,
    };
    let mut mat = None;
    let mut seen = (None, false);
    let mut frame = |events: Vec<Event>, t: f64, b: &mut Board| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(620.0, 900.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            mat = Some(Mat::column(ui.cursor().min, ui.available_width()));
            formation_board(ui, b, &links, Some(0.5), crate::Lang::En, true, true);
            let big = ui.data(|d| d.get_temp::<PaneAsk>(pane_id()).is_some());
            let sel = ui.data(|d| {
                d.get_temp::<Option<u32>>(Id::unique("board-select"))
                    .flatten()
            });
            seen = (take_touched(ui).map(|c| c.number), big || sel.is_some());
        })
        .textures_delta
        .clear();
        (mat.unwrap(), seen)
    };
    let (m, _) = frame(vec![], 0.0, &mut b);
    let at = m.slot_rects(&b.spots)[0].center();
    let press = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    // resting on the card past the hover delay: nothing shown, nothing touched
    frame(vec![Event::PointerMoved(at)], 0.1, &mut b);
    assert_eq!(frame(vec![], 2.0, &mut b).1, (None, false));
    // pressed: touched; released (a click): no selection, no big card
    assert_eq!(frame(vec![press(true)], 2.1, &mut b).1, (Some(101), false));
    assert!(!frame(vec![press(false)], 2.2, &mut b).1.1);
    assert_eq!(frame(vec![], 4.0, &mut b).1, (None, false));
    // a tap pressed and let go in one frame: touched too
    assert_eq!(
        frame(vec![press(true), press(false)], 5.0, &mut b).1,
        (Some(101), false)
    );
}

/// A selected card lets go 3 s after the last touch of the board (a press on it, the mouse on the selected card, a
/// key); a drag in progress is never cut off, however long it lasts.
#[test]
fn a_selection_lets_go_three_seconds_after_the_last_touch() {
    use egui::{Event, Key, PointerButton};
    use forza::cards::Placed;
    let placed = |slot, no, x| Placed {
        slot,
        no,
        x,
        y: 0.0,
    };
    let mut b = forza::cards::board(&[placed(1, 101, -0.6), placed(2, 102, 0.6)], &[]);
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    let links = Links {
        club: [((101, 102), 200)].into_iter().collect(),
        affinity: None,
    };
    type Frame<'a> = dyn FnMut(Vec<Event>, f64, &mut Board) -> (Mat, (Option<u32>, bool)) + 'a;
    // (the selected card, a drag in hand) after one frame at time `t`
    let mut frame = |events: Vec<Event>, t: f64, b: &mut Board| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(620.0, 900.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let (mut mat, mut seen) = (None, (None, false));
        ctx.run_ui(raw, |ui| {
            mat = Some(Mat::column(ui.cursor().min, ui.available_width()));
            formation_board(ui, b, &links, Some(0.5), crate::Lang::En, true, false);
            let sel = ui.data(|d| {
                d.get_temp::<Option<u32>>(Id::unique("board-select"))
                    .flatten()
            });
            seen = (
                sel,
                egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx()),
            );
        })
        .textures_delta
        .clear();
        (mat.unwrap(), seen)
    };
    let (m, _) = frame(vec![], 0.0, &mut b);
    let rects = m.slot_rects(&b.spots);
    let slot = |no| (0..11).find(|&s| b.number(s) == Some(no)).unwrap();
    let (at, other) = (rects[slot(101)].center(), rects[slot(102)].center());
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let click = |t: f64, b: &mut Board, frame: &mut Frame| {
        frame(vec![Event::PointerMoved(at)], t, b);
        frame(vec![button(at, true)], t + 0.1, b);
        let sel = frame(vec![button(at, false)], t + 0.2, b).1.0;
        assert_eq!(sel, Some(101), "a click selects");
    };
    let sel = |events, t, b: &mut Board, frame: &mut Frame| frame(events, t, b).1.0;

    // the mouse leaves: 3 s after the click, not before
    click(0.0, &mut b, &mut frame);
    assert_eq!(
        sel(vec![Event::PointerGone], 0.3, &mut b, &mut frame),
        Some(101)
    );
    assert_eq!(sel(vec![], 2.1, &mut b, &mut frame), Some(101));
    assert_eq!(sel(vec![], 3.4, &mut b, &mut frame), None);
    // a key restarts the count
    click(10.0, &mut b, &mut frame);
    sel(vec![Event::PointerGone], 10.3, &mut b, &mut frame);
    let key = Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    };
    assert_eq!(sel(vec![key], 12.0, &mut b, &mut frame), Some(101));
    assert_eq!(sel(vec![], 14.9, &mut b, &mut frame), Some(101));
    assert_eq!(sel(vec![], 15.1, &mut b, &mut frame), None);
    // the mouse resting on the selected card keeps it; once on another card the count starts
    click(20.0, &mut b, &mut frame);
    assert_eq!(sel(vec![], 29.0, &mut b, &mut frame), Some(101));
    assert_eq!(
        sel(vec![Event::PointerMoved(other)], 29.5, &mut b, &mut frame),
        Some(101)
    );
    assert_eq!(sel(vec![], 31.9, &mut b, &mut frame), Some(101));
    assert_eq!(sel(vec![], 32.2, &mut b, &mut frame), None);
    // a drag in hand for 20 s never cancels it; 3 s after the drop it goes
    click(40.0, &mut b, &mut frame);
    frame(vec![Event::PointerMoved(at)], 41.0, &mut b);
    frame(vec![button(at, true)], 41.1, &mut b);
    for (i, dx) in [10.0, 30.0, 50.0, 30.0].into_iter().enumerate() {
        frame(
            vec![Event::PointerMoved(at + vec2(dx, 0.0))],
            41.2 + i as f64 * 0.1,
            &mut b,
        );
    }
    let held = frame(vec![], 60.0, &mut b).1;
    assert_eq!(held, (Some(101), true), "still in hand, still selected");
    let drop_at = at + vec2(30.0, 0.0);
    frame(
        vec![button(drop_at, false), Event::PointerGone],
        60.1,
        &mut b,
    );
    assert_eq!(sel(vec![], 62.9, &mut b, &mut frame), Some(101));
    assert_eq!(sel(vec![], 63.3, &mut b, &mut frame), None);
}

/// [`row_height`] is what the board window's board (item spacing 4, bench under the pitch) takes, bench included.
#[test]
fn row_height_holds_the_bench() {
    let mut b = forza::cards::board(&[], &[]);
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    let mut used = Rect::NOTHING;
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 1400.0))),
        ..Default::default()
    };
    ctx.run_ui(raw, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let top = ui.cursor().top();
        formation_board(
            ui,
            &mut b,
            &Links::default(),
            None,
            crate::Lang::En,
            false,
            true,
        );
        used = ui.min_rect().with_min_y(top);
    })
    .textures_delta
    .clear();
    assert!(
        (used.height() - row_height(used.width())).abs() < 0.5,
        "{used:?}"
    );
}

/// A card dragged off the board and let go leaves the table, and says which card and where it was.
#[test]
fn a_card_dragged_off_the_board_is_taken_off() {
    use egui::{Event, PointerButton};
    use forza::cards::Placed;
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: 0.0,
        y: 0.0,
    }];
    let mut b = forza::cards::board(&before, &[]);
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut pitch = Rect::NOTHING;
    let mut removed = None;
    let mut frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 1400.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            *pitch = pitch_rect(ui.cursor().min, ui.available_width());
            formation_board(
                ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                false,
                false,
            );
            removed = removed.take().or(take_removed(ui));
        })
        .textures_delta
        .clear();
    };
    frame(vec![], &mut b, &mut pitch);
    let spot = b.spots[0];
    let at = spot_rect(pitch, spot).center();
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    // a right-click takes nothing off (a touch screen's long press is one)
    for pressed in [true, false] {
        let right = Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![Event::PointerMoved(at), right], &mut b, &mut pitch);
    }
    frame(vec![], &mut b, &mut pitch);
    assert_eq!(b.slots[0], Some(0), "a right-click took the card off");
    frame(
        vec![Event::PointerMoved(at), press(at, true)],
        &mut b,
        &mut pitch,
    );
    // out through the bottom of the pitch, past the bench, and let go there
    let out = pos2(at.x, 1300.0);
    for p in [at + vec2(0.0, 30.0), at + vec2(0.0, 200.0), out] {
        frame(vec![Event::PointerMoved(p)], &mut b, &mut pitch);
    }
    assert_eq!(b.slots[0], Some(0), "still on the table while held");
    frame(vec![press(out, false)], &mut b, &mut pitch);
    assert_eq!(b.slots.iter().flatten().count(), 0);
    let r = removed.expect("the removal is reported");
    assert_eq!((r.card.number, r.slot, r.spot), (101, 0, spot));
}

/// The pop-out board has a footer under the pitch inside its window: a release below the pitch but inside the
/// window leaves the card on the table, one outside the window takes it off and reports where.
#[test]
fn the_pop_out_board_takes_a_card_off_only_outside_its_window() {
    use egui::{Event, PointerButton};
    use forza::cards::Placed;
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: 0.0,
        y: 0.9,
    }];
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let run = |release: Pos2| {
        let mut b = forza::cards::board(&before, &[]);
        let mut removed = None;
        let mut frame = |events: Vec<Event>, b: &mut Board| {
            let mut mat = None;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 1000.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| {
                mat = Some(Mat::column(ui.cursor().min, ui.available_width()));
                formation_board(ui, b, &Links::default(), None, crate::Lang::En, true, true);
                removed = removed.take().or(take_removed(ui));
            })
            .textures_delta
            .clear();
            mat.unwrap()
        };
        let mat = frame(vec![], &mut b);
        let at = mat.slot_rects(&b.spots)[0].center();
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![Event::PointerMoved(at), press(at, true)], &mut b);
        for p in [at + vec2(0.0, 30.0), at + vec2(0.0, 150.0), release] {
            frame(vec![Event::PointerMoved(p)], &mut b);
        }
        frame(vec![press(release, false)], &mut b);
        (b.slots.iter().flatten().count(), removed)
    };
    // below the pitch (a card's width past it), inside the window: the footer's place
    let (n, removed) = run(pos2(250.0, 990.0));
    assert_eq!((n, removed.is_none()), (1, true));
    let (n, removed) = run(pos2(250.0, 1100.0));
    assert_eq!(n, 0);
    assert_eq!(removed.expect("taken off").card.number, 101);
}

/// During a match, a card let go off the board stays on the table at its spot from before the lift, and the
/// board says so ([`take_kept`]); no removal is reported.
#[test]
fn a_card_dragged_off_the_board_stays_during_a_match() {
    use egui::{Event, PointerButton};
    use forza::cards::Placed;
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: 0.0,
        y: 0.0,
    }];
    let mut b = forza::cards::board(&before, &[]);
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut pitch = Rect::NOTHING;
    let (mut removed, mut kept) = (None, None);
    let mut frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 1400.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            *pitch = pitch_rect(ui.cursor().min, ui.available_width());
            keep_on_table(ui, true);
            formation_board(
                ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                false,
                false,
            );
            removed = removed.take().or(take_removed(ui));
            kept = kept.take().or(take_kept(ui));
        })
        .textures_delta
        .clear();
    };
    frame(vec![], &mut b, &mut pitch);
    let spot = b.spots[0];
    let at = spot_rect(pitch, spot).center();
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(
        vec![Event::PointerMoved(at), press(at, true)],
        &mut b,
        &mut pitch,
    );
    let out = pos2(at.x, 1300.0);
    for p in [at + vec2(0.0, 30.0), at + vec2(0.0, 200.0), out] {
        frame(vec![Event::PointerMoved(p)], &mut b, &mut pitch);
    }
    frame(vec![press(out, false)], &mut b, &mut pitch);
    assert_eq!(b.slots[0], Some(0), "the card left the table");
    assert_eq!(b.spots[0], spot, "the card did not snap back");
    assert!(removed.is_none(), "a removal was reported");
    assert_eq!(kept, Some(101));
}

/// A full rub of the leftmost bench card from its outer edge, let go at its far end, keeps the card on the table.
#[test]
fn a_rubbed_bench_card_is_not_taken_off() {
    use egui::{Event, PointerButton};
    use forza::cards::{BENCH_X, BENCH_Y, Placed};
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: BENCH_X,
        y: BENCH_Y[0],
    }];
    let mut b = forza::cards::board(&before, &[]);
    assert_eq!(b.slots[11], Some(0));
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut pitch = Rect::NOTHING;
    let frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
        let raw = egui::RawInput {
            // the board 16 points in from the window's left, as in the panel's strip
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(532.0, 900.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            let r = ui.max_rect().shrink(16.0);
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
            *pitch = pitch_rect(ui.cursor().min, ui.available_width());
            formation_board(
                &mut ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                false,
                false,
            );
        })
        .textures_delta
        .clear();
    };
    frame(vec![], &mut b, &mut pitch);
    // grabbed at its outer (left) edge, then rubbed a little over a card's width further out
    let at = slot_rects(pitch, pitch.bottom() + 8.0, &b.spots)[11].left_center() + vec2(2.0, 0.0);
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(
        vec![Event::PointerMoved(at), press(at, true)],
        &mut b,
        &mut pitch,
    );
    // a rub overshoots a little past a card's width
    let far = at - vec2(card_size(pitch.width()).x * 1.2, 0.0);
    frame(vec![Event::PointerMoved(far)], &mut b, &mut pitch);
    frame(vec![press(far, false)], &mut b, &mut pitch);
    assert_eq!(b.slots[11], Some(0));
}

/// A bench card in hand rubs inside its box: x and y move together (the game reads dx * dy) and stay in its band;
/// a still one lies on its spot.
#[test]
fn a_held_bench_card_moves_inside_its_box() {
    use egui::{Event, PointerButton};
    use forza::cards::{BENCH_X, BENCH_Y, Placed, table_after};
    let before = vec![Placed {
        slot: 1,
        no: 101,
        x: BENCH_X,
        y: BENCH_Y[2],
    }];
    let mut b = forza::cards::board(&before, &[]);
    assert_eq!(b.slots[13], Some(0));
    let ctx = egui::Context::default();
    theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let mut pitch = Rect::NOTHING;
    let frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 900.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            *pitch = pitch_rect(ui.cursor().min, ui.available_width());
            formation_board(
                ui,
                b,
                &Links::default(),
                Some(3.0),
                crate::Lang::En,
                false,
                false,
            );
        })
        .textures_delta
        .clear();
    };
    frame(vec![], &mut b, &mut pitch);
    // still: on its spot
    assert_eq!(table_after(&before, &b)[0].y, BENCH_Y[2]);
    let at = slot_rects(pitch, pitch.bottom() + 8.0, &b.spots)[13].center();
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(
        vec![Event::PointerMoved(at), press(at, true)],
        &mut b,
        &mut pitch,
    );
    let mut lines = Vec::new();
    for dx in [10.0, 20.0, 5.0, 25.0, 0.0, 30.0] {
        frame(
            vec![Event::PointerMoved(at + vec2(dx, 0.0))],
            &mut b,
            &mut pitch,
        );
        let l = table_after(&before, &b)[0];
        // the game's band: floor((y + 0.68) / 0.34)
        assert_eq!(((l.y + 0.68) / 0.34).floor(), 2.0, "{l:?}");
        assert!(l.x > 1.16);
        lines.push((l.x, l.y));
    }
    let moves = lines.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(moves >= 4, "{lines:?}");
    assert!(
        lines
            .windows(2)
            .all(|w| (w[1].0 - w[0].0) * (w[1].1 - w[0].1) >= 0.0)
    );
    // let go: back on the spot
    frame(vec![press(at, false)], &mut b, &mut pitch);
    let l = table_after(&before, &b)[0];
    assert_eq!((l.x, l.y), (BENCH_X, BENCH_Y[2]));
}

/// A card's tile stays on the pitch at any spot, and lies centred on its spot away from the edges.
#[test]
fn a_card_never_leaves_the_pitch() {
    let pitch = pitch_rect(pos2(0.0, 0.0), 400.0);
    for s in [
        pos2(0.0, 0.0),
        pos2(1.0, 1.0),
        pos2(0.0, 1.0),
        pos2(1.0, 0.0),
    ] {
        assert!(pitch.contains_rect(spot_rect(pitch, s)), "{s:?}");
    }
    let mid = spot_rect(pitch, pos2(0.5, 0.5)).center();
    assert!((mid - field(pitch).center()).length() < 0.01);
}

/// The keeper's card is drawn lower than any outfield row, on the own goal line, and a card let go there still
/// reads as the keeper (the table's y stays in the keeper's zone).
#[test]
fn the_keeper_sits_lower_and_reads_as_the_keeper() {
    let pitch = pitch_rect(pos2(0.0, 0.0), 400.0);
    let gk = spot_rect(pitch, pos2(0.5, 0.92));
    let back = spot_rect(pitch, pos2(0.5, 0.87));
    assert!(gk.center().y > back.center().y + 20.0);
    assert!(pitch.contains_rect(gk));
    assert_eq!(zone(spot_at(pitch, gk.center())), Position::Gk);
    assert_eq!(zone(spot_at(pitch, back.center())), Position::Df);
}

/// The game's lines (area.adb's areas 10 and 11, FUN_004db340's keeper box), in table coordinates: x -1..1,
/// y -1 (far goal line) ..1 (own); edges inclusive, FW first.
#[test]
fn the_lines_are_the_games() {
    let at = |x: f32, y: f32| zone(pos2((x + 1.0) / 2.0, (y + 1.0) / 2.0));
    // FW down to 25 m past halfway (y = -25/52.5), MF down to 15 m inside the own half (y = 15/52.5)
    assert_eq!(at(0.0, -25.0 / 52.5 - 0.001), Position::Fw);
    assert_eq!(at(0.0, -25.0 / 52.5 + 0.01), Position::Mf);
    assert_eq!(at(0.9, 15.0 / 52.5 - 0.001), Position::Mf);
    assert_eq!(at(0.0, 15.0 / 52.5 + 0.01), Position::Df);
    // the keeper's box: y past 0.8, x inside +-0.2; beside it, DF
    assert_eq!(at(0.1, 0.85), Position::Gk);
    assert_eq!(at(0.3, 0.85), Position::Df);
    assert_eq!(at(0.0, 0.79), Position::Df);
}

#[test]
fn the_packs_real_stats_replace_the_catalogues() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join(hq::FILE),
        "7\tFW\t19\t6\t17\t13\t18\t15\t88\n",
    )
    .unwrap();
    let ctx = egui::Context::default();
    set_picture_dir(&ctx, Some(d.path().to_path_buf()));
    let seven = card(7);
    let c = real(&ctx, &seven);
    assert_eq!(
        (c.pos, c.stats, c.total),
        (Position::Fw, [19, 6, 17, 13, 18, 15], 88)
    );
    // a card the pack lacks, and no pack at all: the game's numbers
    assert!(matches!(
        real(&ctx, &card(8)),
        std::borrow::Cow::Borrowed(_)
    ));
    set_picture_dir(&ctx, None);
    assert!(matches!(
        real(&ctx, &card(7)),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// The downloaded 664x911 pictures, browsed one a pass past 300 cards as in the panel: each texture shrunk to fit
/// [`HQ_MAX`], and the textures kept stay within [`PICTURE_BYTES`] plus the cards on screen. Prints the peak.
#[test]
fn downloaded_pictures_stay_within_the_byte_bound() {
    let dir = tempfile::tempdir().unwrap();
    let big = image::RgbImage::from_fn(664, 911, |x, y| image::Rgb([x as u8, y as u8, 7]));
    big.save(dir.path().join("0_hq.jpg")).unwrap();
    let n = 300u32;
    for i in 1..=n {
        std::fs::copy(
            dir.path().join("0_hq.jpg"),
            dir.path().join(format!("{i}_hq.jpg")),
        )
        .unwrap();
    }
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    ctx.add_image_loader(Arc::new(HqLoader::default()));
    ctx.options_mut(|o| o.reduce_texture_memory = true);
    // (all, the largest one)
    let hq_bytes = |ctx: &egui::Context| -> (usize, usize) {
        ctx.tex_manager()
            .read()
            .allocated()
            .filter(|(_, m)| m.name.ends_with("_hq.jpg"))
            .map(|(_, m)| {
                assert!(
                    m.size[0] as u32 <= HQ_MAX.0 && m.size[1] as u32 <= HQ_MAX.1,
                    "{:?}",
                    m.size
                );
                m.size[0] * m.size[1] * m.bytes_per_pixel
            })
            .fold((0, 0), |(all, one), b| (all + b, one.max(b)))
    };
    let (mut peak, mut one) = (0, 0);
    for i in 0..n {
        let mut ready = false;
        for _ in 0..4000 {
            ctx.run_ui(egui::RawInput::default(), |_| {
                ready = card_picture_texture(&ctx, dir.path(), i).is_some();
                let _ = card_picture_texture(&ctx, dir.path(), n); // a card on screen all along
            })
            .textures_delta
            .clear();
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(ready, "card {i} loaded");
        let (all, largest) = hq_bytes(&ctx);
        (peak, one) = (peak.max(all), one.max(largest));
    }
    println!(
        "downloaded pictures: {} KB a texture, peak {:.1} MB of textures for {n} cards browsed",
        one / 1024,
        peak as f64 / 1_048_576.0
    );
    assert!(peak <= PICTURE_BYTES + 2 * one, "{peak} bytes");
    assert!(
        peak >= PICTURE_BYTES - 2 * one,
        "the bound is used: {peak} bytes"
    );
}

/// Past [`PICTURES_KEPT`] distinct pictures, the oldest are forgotten (their textures freed), while a picture
/// drawn in every pass keeps its first texture.
#[test]
fn card_pictures_stay_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let n = PICTURES_KEPT as u32 + 40;
    // the name tried first: no missed name leaves its error in the loaders
    let px = image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]));
    for i in 0..=n {
        px.save(dir.path().join(format!("{i}_hq.jpg"))).unwrap();
    }
    // as the panel's contexts: a picture's file and decoded pixels go once its texture exists
    let new_ctx = || {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.add_image_loader(Arc::new(HqLoader::default()));
        ctx.options_mut(|o| o.reduce_texture_memory = true);
        ctx
    };
    fn pass(ctx: &egui::Context, mut draw: impl FnMut()) {
        ctx.run_ui(egui::RawInput::default(), |_| draw())
            .textures_delta
            .clear();
    }
    let id =
        |ctx: &egui::Context, i: u32| card_picture_texture(ctx, dir.path(), i).map(|(id, _)| id);

    // browsing: one new card a pass, one card on screen all along and drawn last in each pass
    let ctx = new_ctx();
    let still = n;
    let mut first = None;
    for i in 0..n {
        // each card until it loads (the loaders read and decode on threads)
        let mut ready = false;
        for _ in 0..2000 {
            pass(&ctx, || {
                ready = id(&ctx, i).is_some();
                if let Some(t) = id(&ctx, still) {
                    assert_eq!(*first.get_or_insert(t), t, "the visible card reloaded");
                }
            });
            if ready && first.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(ready, "card {i} loaded");
    }
    let kept = ctx
        .data(|d| d.get_temp::<RecentPictures>(Id::unique("card-pictures-recent")))
        .unwrap()
        .0;
    assert!(kept.len() <= PICTURES_KEPT, "{} kept", kept.len());
    assert!(kept.keys().any(|u| u.ends_with(&format!("{still}_hq.jpg"))));
    // the textures: the kept pictures and the font atlas
    let textures = ctx.tex_manager().read().num_allocated();
    assert!(textures <= PICTURES_KEPT + 1, "{textures} textures");
    // no file or decoded copy beside the textures
    let loaders = ctx.loaders();
    let copies = loaders
        .bytes
        .lock()
        .iter()
        .map(|l| l.byte_size())
        .sum::<usize>()
        + loaders
            .image
            .lock()
            .iter()
            .map(|l| l.byte_size())
            .sum::<usize>();
    assert_eq!(copies, 0, "bytes kept beside the textures");

    // more cards on screen than the bound: all stay, none reloads
    let ctx = new_ctx();
    let mut ids = vec![None; n as usize];
    for _ in 0..4000 {
        pass(&ctx, || {
            for i in 0..n {
                ids[i as usize] = id(&ctx, i);
            }
        });
        if ids.iter().all(Option::is_some) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(ids.iter().all(Option::is_some), "all cards loaded");
    for _ in 0..3 {
        pass(&ctx, || {
            for i in 0..n {
                assert_eq!(id(&ctx, i), ids[i as usize], "card {i} reloaded");
            }
        });
    }
}

/// The box takes the pitch cards whose centre lies in it, never a bench slot or an empty one.
#[test]
fn a_box_takes_the_pitch_cards_whose_centre_is_in_it() {
    let mut rects = [Rect::NOTHING; 16];
    for (s, r) in rects.iter_mut().enumerate() {
        *r = Rect::from_center_size(pos2(s as f32 * 10.0, 0.0), vec2(30.0, 40.0));
    }
    // 0..=4 lie in it by their centre (0 and 4 only partly), 2 is empty, the bench 11..=15 lies past it
    let area = Rect::from_min_max(pos2(-1.0, -1.0), pos2(41.0, 1.0));
    assert_eq!(boxed(&rects, |s| s != 2, area), [0, 1, 3, 4]);
    let all = Rect::from_min_max(pos2(-1.0, -1.0), pos2(500.0, 1.0));
    assert_eq!(boxed(&rects, |_| true, all), (0..11).collect::<Vec<_>>());
    // an edge brushed, the centre outside: not taken
    let brush = Rect::from_min_max(pos2(12.0, -1.0), pos2(14.0, 1.0));
    assert!(boxed(&rects, |_| true, brush).is_empty());
}

/// The group's offset is clamped once: every spot stays in 0..1 and the shape is kept.
#[test]
fn a_group_offset_keeps_every_member_on_the_pitch() {
    let o = [pos2(0.2, 0.3), pos2(0.5, 0.6), pos2(0.4, 0.9)];
    let moved = |want: Vec2| -> Vec<Vec2> {
        group_spots(&o, want)
            .iter()
            .zip(o)
            .map(|(s, o)| *s - o)
            .collect()
    };
    for (want, by) in [
        (vec2(0.1, 0.05), vec2(0.1, 0.05)),
        (vec2(-0.9, 0.9), vec2(-0.2, 0.1)),
        (vec2(0.9, -0.9), vec2(0.5, -0.3)),
    ] {
        assert!(moved(want).iter().all(|d| close(*d, by)), "{want:?}");
    }
    // spots past 0..1 from an odd table: brought onto the pitch, then moved; every spot stays in 0..1
    let odd = [pos2(-0.1, 0.5), pos2(1.1, 0.5), pos2(0.5, 1.2)];
    for want in [vec2(0.3, 0.0), vec2(-0.3, -0.4), vec2(0.0, 0.4)] {
        let to = group_spots(&odd, want);
        let r = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        assert!(to.iter().all(|s| r.contains(*s)), "{want:?}: {to:?}");
    }
}

/// A [`formation_board`] driven by raw input, the group gate on or off.
struct Rig {
    ctx: egui::Context,
    pitch: Rect,
    on: bool,
    keep: bool,
    removed: Option<Removed>,
}

impl Rig {
    fn new(on: bool, b: &mut Board) -> Rig {
        let ctx = egui::Context::default();
        theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let mut r = Rig {
            ctx,
            pitch: Rect::NOTHING,
            on,
            keep: false,
            removed: None,
        };
        r.frame(b, vec![], egui::Modifiers::NONE);
        r
    }

    fn frame(&mut self, b: &mut Board, events: Vec<egui::Event>, mods: egui::Modifiers) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 1400.0))),
            events: [egui::Event::ModifiersChanged(mods)]
                .into_iter()
                .chain(events)
                .collect(),
            ..Default::default()
        };
        let (on, keep) = (self.on, self.keep);
        let (mut pitch, mut removed) = (self.pitch, None);
        self.ctx
            .run_ui(raw, |ui| {
                pitch = pitch_rect(ui.cursor().min, ui.available_width());
                keep_on_table(ui, keep);
                group_moves(ui, on);
                formation_board(
                    ui,
                    b,
                    &Links::default(),
                    None,
                    crate::Lang::En,
                    false,
                    false,
                );
                removed = take_removed(ui);
            })
            .textures_delta
            .clear();
        self.pitch = pitch;
        self.removed = self.removed.take().or(removed);
    }

    /// A spot's screen point.
    fn at(&self, spot: Pos2) -> Pos2 {
        spot_rect(self.pitch, spot).center()
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn drag(&mut self, b: &mut Board, from: Pos2, to: Pos2) {
        let none = egui::Modifiers::NONE;
        let ev = vec![egui::Event::PointerMoved(from), Rig::press(from, true)];
        self.frame(b, ev, none);
        for i in 1..=6 {
            let p = from + (to - from) * (i as f32 / 6.0);
            self.frame(b, vec![egui::Event::PointerMoved(p)], none);
        }
        self.frame(b, vec![Rig::press(to, false)], none);
        self.frame(b, vec![], none);
    }

    fn click(&mut self, b: &mut Board, at: Pos2, mods: egui::Modifiers) {
        let ev = vec![egui::Event::PointerMoved(at), Rig::press(at, true)];
        self.frame(b, ev, mods);
        self.frame(b, vec![Rig::press(at, false)], mods);
        self.frame(b, vec![], mods);
    }

    fn group(&self) -> Vec<u32> {
        let mut g = self
            .ctx
            .data(|d| d.get_temp::<Vec<u32>>(group_id()))
            .unwrap_or_default();
        g.sort_unstable();
        g
    }
}

/// Cards 1..=4 in a square on pitch slots 0..=3, card 5 right of it on slot 4, card 6 on the first bench box.
fn square() -> Board {
    let mut b = Board {
        cards: (1..=6).map(card).collect(),
        slots: [None; 16],
        spots: [pos2(0.5, 0.5); 11],
        rub: None,
    };
    for s in 0..5 {
        b.slots[s] = Some(s);
    }
    b.slots[11] = Some(5);
    let at = [
        (0.2, 0.25),
        (0.45, 0.25),
        (0.2, 0.45),
        (0.45, 0.45),
        (0.7, 0.25),
    ];
    for (s, (x, y)) in at.into_iter().enumerate() {
        b.spots[s] = pos2(x, y);
    }
    b
}

/// The box over the square from open pitch.
fn box_square(r: &mut Rig, b: &mut Board) {
    let (from, to) = (r.at(pos2(0.05, 0.08)), r.at(pos2(0.6, 0.55)));
    r.drag(b, from, to);
}

fn close(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn a_box_over_four_cards_moves_them_together() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    box_square(&mut r, &mut b);
    assert_eq!(r.group(), [1, 2, 3, 4]);
    let was = b.spots;
    let from = r.at(was[0]);
    r.drag(&mut b, from, from + vec2(30.0, 20.0));
    let by = b.spots[0] - was[0];
    assert!(by.x > 0.05 && by.y > 0.02, "{by:?}");
    for (s, (now, at)) in b.spots.iter().zip(was).enumerate().take(4) {
        assert!(close(*now - at, by), "slot {s}: {now:?}");
    }
    assert_eq!(b.spots[4], was[4], "the card outside the box moved");
    assert_eq!(b.slots, square().slots);
}

#[test]
fn a_group_stops_at_the_pitch_edge_together() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    box_square(&mut r, &mut b);
    let was = b.spots;
    let from = r.at(was[0]);
    // its centre ends left of the field, still on the drawn pitch
    r.drag(&mut b, from, pos2(r.pitch.left() + 16.0, from.y));
    let min_x = (0..4).map(|s| b.spots[s].x).fold(1.0, f32::min);
    assert_eq!(min_x, 0.0);
    for s in 1..4 {
        assert!(
            close(b.spots[s] - b.spots[0], was[s] - was[0]),
            "slot {s}: the shape changed"
        );
    }
}

#[test]
fn a_group_drop_never_swaps() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    box_square(&mut r, &mut b);
    // card 1 let go on card 5: no swap, it lies there over it
    let (from, onto) = (r.at(b.spots[0]), r.at(b.spots[4]));
    r.drag(&mut b, from, onto);
    assert_eq!(b.slots, square().slots, "the group swapped");
    assert!(
        close(b.spots[0].to_vec2(), b.spots[4].to_vec2()),
        "{:?}",
        b.spots[0]
    );
}

#[test]
fn bench_cards_never_join_a_group() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    let bench = r
        .ctx
        .read_response(Id::unique(("board-slot", 11)))
        .expect("the bench card")
        .rect
        .center();
    // a box from open pitch down over the bench card, then a Ctrl-click on it
    let from = pos2(r.pitch.left() + 2.0, r.at(pos2(0.05, 0.08)).y);
    let to = pos2(r.at(pos2(0.95, 0.5)).x, bench.y + 10.0);
    r.drag(&mut b, from, to);
    assert_eq!(r.group(), [1, 2, 3, 4, 5]);
    r.click(&mut b, bench, egui::Modifiers::COMMAND);
    assert_eq!(r.group(), [1, 2, 3, 4, 5], "the bench card joined");
    assert_eq!(b.slots, square().slots);
}

#[test]
fn ctrl_click_adds_and_removes_a_card_and_a_click_on_open_pitch_clears() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    let (one, five) = (r.at(b.spots[0]), r.at(b.spots[4]));
    r.click(&mut b, one, egui::Modifiers::COMMAND);
    r.click(&mut b, five, egui::Modifiers::SHIFT);
    assert_eq!(r.group(), [1, 5]);
    // no card shown big by a toggle
    assert!(
        r.ctx
            .data(|d| d.get_temp::<Option<u32>>(Id::unique("board-pin")))
            .flatten()
            .is_none()
    );
    r.click(&mut b, one, egui::Modifiers::COMMAND);
    assert_eq!(r.group(), [5]);
    box_square(&mut r, &mut b);
    assert_eq!(r.group(), [1, 2, 3, 4]);
    r.click(&mut b, r.at(pos2(0.05, 0.08)), egui::Modifiers::NONE);
    assert!(r.group().is_empty());
    // a drag now moves one card only
    let was = b.spots;
    r.drag(&mut b, one, one + vec2(30.0, 0.0));
    assert_ne!(b.spots[0], was[0]);
    assert_eq!(b.spots[1..], was[1..]);
}

/// The info box (#333): the card rested on previews; a click pins it; another card rested on previews beside the
/// pin, which stays; a click on the pinned card again or a close lets it go; nothing shows while a card or a box
/// is in hand.
#[test]
fn the_info_box_previews_a_rested_card_and_pins_a_clicked_one() {
    assert_eq!(boxes(None, Some(7), false), (Some(7), None), "a preview");
    assert_eq!(boxes(None, None, false), (None, None));
    let pin = next_pin(None, PinEvent::Click(7));
    assert_eq!(pin, Some(7), "a click pins");
    assert_eq!(
        boxes(pin, Some(7), false),
        (None, Some(7)),
        "pinned, not previewed too"
    );
    assert_eq!(
        boxes(pin, Some(8), false),
        (Some(8), Some(7)),
        "the pin stays"
    );
    assert_eq!(next_pin(pin, PinEvent::Click(8)), Some(8), "another click");
    assert_eq!(
        next_pin(pin, PinEvent::Click(7)),
        None,
        "a click on it again"
    );
    assert_eq!(next_pin(pin, PinEvent::Close), None, "Esc, a press off it");
    assert_eq!(boxes(pin, Some(8), true), (None, None), "in hand: nothing");
}

/// The pin on the board: a click pins a card, a click on it again or on open pitch lets it go, and a card taken off the table takes its pin with it.
#[test]
fn a_pin_goes_with_a_click_off_it_or_its_card() {
    let none = egui::Modifiers::NONE;
    let mut b = square();
    let mut r = Rig::new(false, &mut b);
    let pin = |r: &Rig| {
        r.ctx
            .data(|d| d.get_temp::<Option<u32>>(Id::unique("board-pin")))
            .flatten()
    };
    let one = r.at(b.spots[0]);
    r.click(&mut b, one, none);
    assert_eq!(pin(&r), Some(1));
    r.click(&mut b, one, none);
    assert_eq!(pin(&r), None, "a click on it again");
    r.click(&mut b, one, none);
    r.click(&mut b, r.at(pos2(0.05, 0.08)), none);
    assert_eq!(pin(&r), None, "a click on open pitch");
    r.click(&mut b, one, none);
    r.drag(&mut b, one, one + vec2(2000.0, 0.0));
    assert!(r.removed.is_some(), "taken off");
    assert_eq!(pin(&r), None, "the pin left with its card");
}

#[test]
fn a_group_let_go_off_the_board_goes_back() {
    for keep in [false, true] {
        let mut b = square();
        let mut r = Rig::new(true, &mut b);
        r.keep = keep;
        box_square(&mut r, &mut b);
        let was = b.spots;
        let from = r.at(was[0]);
        r.drag(&mut b, from, pos2(from.x, 1300.0));
        assert_eq!(b.spots, was, "keep {keep}");
        assert_eq!(b.slots, square().slots, "keep {keep}");
        assert!(r.removed.is_none(), "keep {keep}: taken off");
    }
}

/// A member held by its lower edge and let go with the pointer just over the bench (its centre still on the
/// pitch): the group goes back.
#[test]
fn a_group_let_go_over_the_bench_goes_back() {
    let mut b = square();
    let mut r = Rig::new(true, &mut b);
    box_square(&mut r, &mut b);
    let was = b.spots;
    let from = r.at(was[0]) + vec2(0.0, 40.0);
    r.drag(&mut b, from, pos2(from.x, r.pitch.bottom() + 6.0));
    assert_eq!(b.spots, was);
    assert_eq!(b.slots, square().slots);
    assert!(r.removed.is_none());
}

/// The keeper never joins a group, by the box or by Ctrl-click.
#[test]
fn the_keeper_never_joins_a_group() {
    let mut b = square();
    b.spots[4] = pos2(0.5, 0.95);
    let mut r = Rig::new(true, &mut b);
    let keeper = r
        .ctx
        .read_response(Id::unique(("board-slot", 4)))
        .expect("the keeper")
        .rect
        .center();
    let from = pos2(r.pitch.left() + 2.0, r.at(pos2(0.05, 0.08)).y);
    r.drag(&mut b, from, r.pitch.right_bottom() - vec2(2.0, 2.0));
    assert_eq!(r.group(), [1, 2, 3, 4]);
    r.click(&mut b, keeper, egui::Modifiers::COMMAND);
    assert_eq!(r.group(), [1, 2, 3, 4], "the keeper joined");
}

#[test]
fn without_the_gate_the_board_has_no_box() {
    let mut b = square();
    let mut r = Rig::new(false, &mut b);
    box_square(&mut r, &mut b);
    let five = r.at(b.spots[4]);
    r.click(&mut b, five, egui::Modifiers::COMMAND);
    assert!(r.group().is_empty());
    let was = b.spots;
    let from = r.at(was[0]);
    r.drag(&mut b, from, from + vec2(30.0, 0.0));
    assert_eq!(b.spots[1..], was[1..]);
}
