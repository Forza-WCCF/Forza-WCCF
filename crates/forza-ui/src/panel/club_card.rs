use super::*;

// ---- CLUB CARD ----

/// A choice on CLUB CARD that needs a second click within 8 s, then a restart: another card from YOUR CARDS (its file in
/// data\save\cards and its club), a new card, or CLEAR BAD ENDINGS (2017-18: on SETTINGS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClubPick {
    Card { file: String, club: String },
    New,
    Fix,
}

/// The view's row labels and headings the page translates; any other label shows as the view writes it.
const CLUB_LABELS: [(&str, &str); 16] = [
    ("CLUB", "club.row_club"),
    ("MANAGER", "club.row_manager"),
    ("CONTRACT", "club.row_contract"),
    ("SALARY", "club.row_salary"),
    ("LEAGUE", "club.row_league"),
    ("ALL MATCHES", "club.row_all_matches"),
    ("PRIZE MONEY", "club.row_prize"),
    ("TRADE RIGHTS", "club.row_trade"),
    ("LAST PLAYED", "club.row_last_played"),
    ("CARD HEALTH", "club.row_health"),
    ("SESSION", "club.row_session"),
    ("BAD ENDINGS", "club.row_bad"),
    ("SAVED COPIES", "club.row_copies"),
    ("CARD SAVED", "club.row_saved"),
    ("BACKUPS", "club.backups"),
    ("SQUAD", "club.squad"),
];

fn club_label(lang: Lang, label: &str) -> String {
    CLUB_LABELS
        .iter()
        .find(|(l, _)| *l == label)
        .map_or_else(|| label.to_owned(), |(_, k)| tr!(lang, k))
}

/// The view's colour letters: w white, g green, y yellow, r red, o orange, d dim.
fn club_colour(c: char) -> Color32 {
    match c {
        'g' => theme::GREEN,
        'y' => theme::GOLD,
        'r' => theme::RED_TEXT,
        'o' => theme::AMBER,
        'd' => theme::TEXT_FAINT,
        _ => theme::TEXT,
    }
}

/// A click on a CLUB CARD choice, as `wccfpanel.c` `club_click`: refused while a restart is asked or a card session
/// is open, NEW refused while the slot holds a blank card; a first click arms it for 8 s, a second asks for it
/// ([`PanelState::club_request`]: the panel DLL writes data\panel.txt and asks for the restart).
pub fn club_click(st: &mut PanelState, pick: ClubPick, now: f64) {
    let lang = st.lang;
    if st.restart_sent {
        st.say(Tone::Notice, "club.msg_asked");
    } else if st.club.session == "open" || st.status.card == CardState::Open {
        st.say(Tone::Refused, "club.session_refused");
    } else if pick == ClubPick::New && st.club.state == "none" {
        st.say(Tone::Notice, "club.msg_blank");
    } else if st
        .club_armed
        .as_ref()
        .is_none_or(|(p, until)| *p != pick || *until <= now)
    {
        let text = match &pick {
            ClubPick::Fix => tr!(lang, "club.msg_arm_fix"),
            ClubPick::New => tr!(lang, "club.msg_arm_new"),
            ClubPick::Card { club, .. } => tr!(lang, "club.msg_arm_play", club = club),
        };
        st.note(Tone::Notice, text);
        st.club_armed = Some((pick, now + 8.0));
    } else {
        st.club_armed = None;
        st.club_request = Some(pick);
    }
}

pub(super) fn club_card(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let accent = st.edition.accent();
    let now = ui.input(|i| i.time);
    let armed = st
        .club_armed
        .clone()
        .filter(|(_, until)| *until > now)
        .map(|(p, until)| {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(until - now));
            p
        });
    let view = st.club.clone();
    let ok = view.state == "ok";
    let mut click = None;
    ui.label(
        egui::RichText::new(tr!(lang, "club.intro"))
            .font(theme::body(13.0))
            .color(theme::TEXT_DIM),
    );
    ui.add_space(6.0);
    let foot = 50.0;
    let h = ui.available_height() - foot;
    ui.allocate_ui(vec2(ui.available_width(), h), |ui| {
        egui_extras::StripBuilder::new(ui)
            .size(egui_extras::Size::exact(420.0))
            .size(egui_extras::Size::remainder())
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.set_max_height(h);
                    // the club card itself: landscape, as the physical card, with the club and its manager
                    let (r, _) =
                        ui.allocate_exact_size(vec2(340.0, 340.0 * 58.0 / 81.0), Sense::hover());
                    let p = ui.painter();
                    let s = egui::Shadow {
                        offset: [0, 10],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    };
                    p.add(s.as_shape(r, 14));
                    widgets::gradient(
                        p,
                        r,
                        14,
                        accent.lerp_to_gamma(theme::DEEP, 0.35),
                        theme::DEEP.lerp_to_gamma(theme::WELL, 0.4),
                        false,
                    );
                    p.rect_stroke(r, 14, Stroke::new(1.5, accent), StrokeKind::Inside);
                    let chip_r = Rect::from_min_size(r.min + vec2(24.0, 58.0), vec2(42.0, 32.0));
                    widgets::gradient(
                        p,
                        chip_r,
                        6,
                        theme::GOLD.lerp_to_gamma(Color32::WHITE, 0.3),
                        theme::GOLD,
                        true,
                    );
                    for k in 1..3 {
                        let y = chip_r.top() + chip_r.height() * k as f32 / 3.0;
                        p.line_segment(
                            [pos2(chip_r.left(), y), pos2(chip_r.right(), y)],
                            Stroke::new(1.0, theme::INK.gamma_multiply(0.5)),
                        );
                    }
                    p.text(
                        r.min + vec2(24.0, 22.0),
                        Align2::LEFT_TOP,
                        tr!(lang, "club.label"),
                        theme::display(16.0),
                        Color32::from_white_alpha(170),
                    );
                    // "KITAKAZE  (founded 2026-10-01)": the name alone on the card
                    let first = |v: Option<&str>| {
                        v.map(|v| v.split("  (").next().unwrap_or(v).trim().to_owned())
                            .filter(|v| !v.is_empty() && v != "-")
                    };
                    if let Some(club) = first(view.row("CLUB")) {
                        let g = widgets::fit_elided(
                            p,
                            &club,
                            theme::display(34.0),
                            Color32::WHITE,
                            300.0,
                        );
                        p.galley(r.min + vec2(24.0, 104.0), g, Color32::WHITE);
                    }
                    if let Some(m) = first(view.row("MANAGER")) {
                        let g = widgets::fit_elided(
                            p,
                            &tr!(lang, "club.manager", name = m),
                            theme::body(15.0),
                            theme::TEXT,
                            300.0,
                        );
                        p.galley(r.min + vec2(24.0, 146.0), g, theme::TEXT);
                    }
                    p.text(
                        r.right_bottom() + vec2(-20.0, -18.0),
                        Align2::RIGHT_BOTTOM,
                        st.edition.label(),
                        theme::display(22.0),
                        accent,
                    );
                    ui.add_space(10.0);
                    // CLEAR BAD ENDINGS (2010-11), under the card while there are some or the last session was cut
                    if st.edition == Edition::E1011
                        && ok
                        && (view.bad_endings() > 0 || view.session == "cut")
                    {
                        let lit = armed == Some(ClubPick::Fix);
                        let key = if lit {
                            "club.clear_again"
                        } else {
                            "club.clear_bad"
                        };
                        if cabinet(ui, vec2(300.0, 40.0), &tr!(lang, key), Cab::Stop, lit).clicked()
                        {
                            click = Some(ClubPick::Fix);
                        }
                        ui.label(
                            egui::RichText::new(tr!(lang, "club.clear_hint"))
                                .font(theme::body(HINT))
                                .color(theme::TEXT_DIM),
                        );
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("club-left")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().interact_size.y = 18.0;
                            if !ok {
                                // no club to show: why, then the backups
                                let (msg, tone) = match view.state.as_str() {
                                    "noview" if st.edition == Edition::E1718 => {
                                        (tr!(lang, "club.none_1718"), Tone::Notice)
                                    }
                                    "noview" => (tr!(lang, "club.noview"), Tone::Notice),
                                    "unreadable" => (view.reason.clone(), Tone::Refused),
                                    _ if view.reason.is_empty() => {
                                        (view.state.clone(), Tone::Notice)
                                    }
                                    _ => (view.reason.clone(), Tone::Notice),
                                };
                                status_line(ui, tone, &msg);
                                if !view.backups.is_empty() {
                                    ui.add_space(8.0);
                                    caps(ui, &tr!(lang, "club.backups"));
                                    for b in &view.backups {
                                        ui.label(
                                            egui::RichText::new(b)
                                                .font(theme::mono(13.0))
                                                .color(theme::TEXT),
                                        );
                                    }
                                    ui.label(
                                        egui::RichText::new(tr!(lang, "club.backups_hint"))
                                            .font(theme::body(HINT))
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                                return;
                            }
                            let mut grid = 0;
                            let mut rows: Vec<(&str, &str, char)> = Vec::new();
                            let mut flush = |ui: &mut Ui, rows: &mut Vec<(&str, &str, char)>| {
                                if rows.is_empty() {
                                    return;
                                }
                                grid += 1;
                                egui::Grid::new(("club-rows", grid))
                                    .num_columns(2)
                                    .spacing(vec2(14.0, 4.0))
                                    .striped(true)
                                    .min_col_width(110.0)
                                    .show(ui, |ui| {
                                        for (l, v, c) in rows.drain(..) {
                                            kv_coloured(
                                                ui,
                                                &club_label(lang, l),
                                                v,
                                                club_colour(c),
                                            );
                                        }
                                    });
                            };
                            for line in &view.lines {
                                match line {
                                    forza_club::Line::Head(t) => {
                                        flush(ui, &mut rows);
                                        ui.add_space(4.0);
                                        caps(ui, &club_label(lang, t));
                                    }
                                    forza_club::Line::Row {
                                        label,
                                        value,
                                        colour,
                                    } => {
                                        rows.push((label, value, *colour));
                                    }
                                }
                            }
                            flush(ui, &mut rows);
                        });
                });
                strip.cell(|ui| {
                    ui.set_max_height(h);
                    egui::ScrollArea::vertical()
                        .id_salt("club-right")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            if ok && !view.squad.is_empty() {
                                caps(ui, &tr!(lang, "club.squad"));
                                egui::Grid::new("club-squad")
                                    .num_columns(7)
                                    .spacing(vec2(12.0, 4.0))
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for k in [
                                            "club.sq_no",
                                            "club.sq_name",
                                            "club.sq_pos",
                                            "club.sq_apps",
                                            "club.sq_goals",
                                            "club.sq_assists",
                                            "club.sq_cond",
                                        ] {
                                            ui.label(
                                                egui::RichText::new(tr!(lang, k))
                                                    .font(theme::display(13.0))
                                                    .color(theme::TEXT_DIM),
                                            );
                                        }
                                        ui.end_row();
                                        for p in &view.squad {
                                            let pos = match p[2].chars().next() {
                                                Some('G') => theme::POS_GK,
                                                Some('D') => theme::POS_DF,
                                                Some('M') => theme::POS_MF,
                                                _ => theme::POS_FW,
                                            };
                                            let injured = p[7] != "-";
                                            let cell = |ui: &mut Ui, t: &str, c: Color32| {
                                                ui.label(
                                                    egui::RichText::new(t)
                                                        .font(theme::body(13.0))
                                                        .color(c),
                                                );
                                            };
                                            cell(ui, &p[0], theme::TEXT);
                                            cell(
                                                ui,
                                                &p[1],
                                                if injured {
                                                    theme::RED_TEXT
                                                } else {
                                                    theme::TEXT
                                                },
                                            );
                                            cell(ui, &p[2], pos);
                                            for v in &p[3..7] {
                                                cell(ui, v, theme::TEXT);
                                            }
                                            ui.end_row();
                                        }
                                    });
                                ui.label(
                                    egui::RichText::new(tr!(lang, "club.injured"))
                                        .font(theme::body(HINT))
                                        .color(theme::TEXT_DIM),
                                );
                                ui.add_space(6.0);
                            }
                            caps(ui, &tr!(lang, "club.your_cards"));
                            if view.wallet.is_empty() {
                                ui.label(
                                    egui::RichText::new(tr!(lang, "club.no_others"))
                                        .font(theme::body(13.0))
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for w in &view.wallet {
                                let pick = ClubPick::Card {
                                    file: w.file.clone(),
                                    club: w.club.clone(),
                                };
                                let lit = armed.as_ref() == Some(&pick);
                                if club_row(ui, &w.club, &w.summary, w.transferred, lit, lang) {
                                    click = Some(pick);
                                }
                            }
                            ui.add_space(4.0);
                            let w = ui.available_width().min(360.0);
                            let lit = armed == Some(ClubPick::New);
                            let key = if lit {
                                "club.new_again"
                            } else {
                                "club.new_card"
                            };
                            if cabinet(ui, vec2(w, 44.0), &tr!(lang, key), Cab::Tactic, lit)
                                .clicked()
                            {
                                click = Some(ClubPick::New);
                            }
                            let open = view.session == "open" || st.status.card == CardState::Open;
                            ui.label(
                                egui::RichText::new(tr!(
                                    lang,
                                    if open {
                                        "club.session_refused"
                                    } else {
                                        "club.switch_hint"
                                    }
                                ))
                                .font(theme::body(HINT))
                                .color(if open {
                                    theme::RED_TEXT
                                } else {
                                    theme::TEXT_DIM
                                }),
                            );
                        });
                });
            });
    });
    if let Some(pick) = click {
        club_click(st, pick, now);
    }
    note_line(ui, &mut st.message);
}

/// A row and its value in the view's colour.
fn kv_coloured(ui: &mut Ui, k: &str, v: &str, c: Color32) {
    ui.label(
        egui::RichText::new(k)
            .color(theme::TEXT_DIM)
            .font(theme::body(13.0)),
    );
    ui.add(egui::Label::new(egui::RichText::new(v).color(c).font(theme::bold(13.0))).truncate());
    ui.end_row();
}

/// One of YOUR CARDS: its club and summary, and PLAY THIS CLUB (CLICK AGAIN while armed), or TRANSFERRED as a word;
/// `true` = PLAY THIS CLUB clicked.
fn club_row(ui: &mut Ui, name: &str, sub: &str, transferred: bool, lit: bool, lang: Lang) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 54.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        r,
        theme::RADIUS_BIG,
        if lit {
            theme::AMBER.gamma_multiply(0.14)
        } else {
            theme::RAISED
        },
    );
    p.rect_stroke(
        r,
        theme::RADIUS_BIG,
        Stroke::new(
            if lit { 2.0 } else { 1.0 },
            if lit { theme::AMBER } else { theme::RIM },
        ),
        StrokeKind::Inside,
    );
    let bw = 170.0;
    let tw = w - bw - 30.0;
    let g = widgets::fit_elided(p, name, theme::display(20.0), theme::TEXT, tw);
    p.galley(pos2(r.left() + 14.0, r.top() + 6.0), g, theme::TEXT);
    let g = widgets::fit_elided(p, sub, theme::body(HINT), theme::TEXT_DIM, tw);
    p.galley(pos2(r.left() + 14.0, r.top() + 31.0), g, theme::TEXT_DIM);
    let b = Rect::from_min_size(pos2(r.right() - bw - 8.0, r.top() + 8.0), vec2(bw, 38.0));
    if transferred {
        let g = widgets::fit_elided(
            p,
            &tr!(lang, "club.transferred"),
            theme::display(15.0),
            theme::TEXT_FAINT,
            bw,
        );
        p.galley(
            Align2::CENTER_CENTER.anchor_size(b.center(), g.size()).min,
            g,
            theme::TEXT_FAINT,
        );
        return false;
    }
    let key = if lit { "club.click_again" } else { "club.play" };
    ui.put(b, |ui: &mut Ui| {
        cabinet(ui, b.size(), &tr!(lang, key), Cab::Tactic, lit)
    })
    .clicked()
}
