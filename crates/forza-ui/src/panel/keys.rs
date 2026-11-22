use super::*;

// ---- KEYS ----

/// The keys and controls the page was given since the last frame, by `wccfpanel.c`'s rules (`keys_key`,
/// `pad_assign_locked`, `pad_tick`): a waiting KEY cell takes an allowed key, and the action that had it takes the
/// waiting one's old key (Esc cancels; Alt or a key that must stay free is refused); a waiting CONTROLLER cell takes the
/// control pressed, swapped the same way. Each change sets `keys_changed` (keys.txt is written).
pub fn keys_input(st: &mut PanelState) {
    let lang = st.lang;
    let label = |i: usize| tr!(lang, ACTIONS[i].0);
    for (vk, alt) in std::mem::take(&mut st.vk_down) {
        if let Some(k) = st.pad_capturing {
            if vk == 0x1B {
                st.pad_capturing = None;
                let now = pad_name(lang, st.pads[k]);
                st.note(
                    Tone::Notice,
                    tr!(lang, "keys.msg_keeps", action = label(k), key = now),
                );
            } else {
                let key = forza::keys::key_label(vk);
                st.note(Tone::Notice, tr!(lang, "keys.msg_is_a_key", key = key));
            }
            continue;
        }
        let Some(k) = st.capturing else { continue };
        let old = st.keys[k];
        let other = (0..ACTIONS.len()).find(|&j| j != k && st.keys[j] == vk);
        if vk == 0x1B || vk == old {
            st.capturing = None;
            let key = key_name(lang, old);
            st.note(
                Tone::Notice,
                tr!(lang, "keys.msg_keeps", action = label(k), key = key),
            );
        } else if alt
            || !forza::keys::fits(k, vk)
            || other.is_some_and(|j| !forza::keys::fits(j, old))
        {
            let key = if alt && vk != 0x12 {
                "Alt".into()
            } else {
                forza::keys::key_label(vk)
            };
            st.note(Tone::Refused, tr!(lang, "keys.msg_refused", key = key));
        } else {
            st.keys[k] = vk;
            if let Some(j) = other {
                st.keys[j] = old;
            }
            (st.capturing, st.changed, st.keys_changed) = (None, Some(k), true);
            let text = match other {
                Some(j) => tr!(
                    lang,
                    "keys.msg_swapped",
                    action = label(k),
                    key = key_name(lang, vk),
                    other = label(j),
                    old = key_name(lang, old)
                ),
                None => tr!(
                    lang,
                    "keys.msg_set",
                    action = label(k),
                    key = key_name(lang, vk)
                ),
            };
            st.note(Tone::Done, text);
        }
    }
    let Some(code) = st.pad_down.take() else {
        return;
    };
    if let Some(k) = st.pad_capturing {
        let old = st.pads[k];
        st.pad_capturing = None;
        if code == old {
            st.note(
                Tone::Notice,
                tr!(
                    lang,
                    "keys.msg_keeps",
                    action = label(k),
                    key = pad_name(lang, code)
                ),
            );
            return;
        }
        let other = (0..ACTIONS.len()).find(|&j| j != k && st.pads[j] == code);
        st.pads[k] = code;
        if let Some(j) = other {
            st.pads[j] = old;
        }
        (st.changed, st.keys_changed) = (Some(k), true);
        let text = match other {
            Some(j) => tr!(
                lang,
                "keys.msg_swapped",
                action = label(k),
                key = pad_name(lang, code),
                other = label(j),
                old = pad_name(lang, old)
            ),
            None => tr!(
                lang,
                "keys.msg_set",
                action = label(k),
                key = pad_name(lang, code)
            ),
        };
        st.note(Tone::Done, text);
    } else if st.capturing.is_some() {
        st.note(
            Tone::Notice,
            tr!(lang, "keys.msg_is_a_control", key = pad_name(lang, code)),
        );
    }
}

fn key_name(lang: Lang, vk: u32) -> String {
    if vk == 0 {
        tr!(lang, "keys.none")
    } else {
        forza::keys::key_label(vk)
    }
}

fn pad_name(lang: Lang, code: u32) -> String {
    if code == 0 {
        tr!(lang, "keys.none")
    } else {
        forza::keys::pad_label(code)
    }
}

pub(super) fn keys(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    keys_input(st);
    ui.label(egui::RichText::new(tr!(lang, "keys.hint")).color(theme::TEXT_DIM));
    if st.pad_names.is_empty() {
        status_line(ui, Tone::Notice, &tr!(lang, "keys.no_controller"));
    } else {
        status_line(
            ui,
            Tone::Done,
            &tr!(
                lang,
                "keys.controller_found",
                name = st.pad_names.join(", ")
            ),
        );
    }
    ui.add_space(4.0);
    let cell_w = 200.0;
    // an open Advanced group scrolls instead of pushing RESET KEYS off the page
    let room = (ui.available_height() - 56.0).max(120.0);
    egui::ScrollArea::vertical()
        .max_height(room)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("keys")
                .num_columns(4)
                .spacing(vec2(16.0, 3.0))
                .striped(false)
                .show(ui, |ui| {
                    caps(ui, &tr!(lang, "keys.action"));
                    ui.label("");
                    caps(ui, &tr!(lang, "keys.keyboard"));
                    caps(ui, &tr!(lang, "keys.controller"));
                    ui.end_row();
                    for i in (0..ACTIONS.len()).filter(|i| !OPERATOR.contains(i)) {
                        if i == forza::keys::PAUSE && !pause_on(st.edition) {
                            continue;
                        }
                        row(ui, st, lang, i, cell_w);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            // the operator's switches: TEST, SERVICE, SW1, SW2
            let mut open = st.keys_advanced;
            widgets::fold(
                ui,
                &tr!(lang, "keys.advanced"),
                "",
                &mut open,
                theme::AMBER,
                |ui| {
                    egui::Grid::new("keys-advanced")
                        .num_columns(4)
                        .spacing(vec2(16.0, 3.0))
                        .striped(false)
                        .show(ui, |ui| {
                            for i in OPERATOR {
                                row(ui, st, lang, i, cell_w);
                                ui.end_row();
                            }
                        });
                },
            );
            st.keys_advanced = open;
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let now = ui.input(|i| i.time);
        let armed = st.keys_reset_armed > now;
        if armed {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    st.keys_reset_armed - now,
                ));
        }
        let label = tr!(
            lang,
            if armed {
                "keys.defaults_again"
            } else {
                "keys.defaults"
            }
        );
        if action(ui, &label).clicked() {
            reset_click(st, now);
        }
        ui.add_space(12.0);
        note_line(ui, &mut st.message);
    });
}

fn row(ui: &mut Ui, st: &mut PanelState, lang: Lang, i: usize, cell_w: f32) {
    let (key, kind) = ACTIONS[i];
    let disabled = key == "cab.coin" && st.edition == Edition::E1011; // free play: COIN is not needed
    let (r, _) = ui.allocate_exact_size(vec2(130.0, 30.0), Sense::hover());
    let lit = st.capturing == Some(i) || st.pad_capturing == Some(i);
    cabinet_face(
        ui.painter(),
        r,
        &tr!(lang, key),
        kind,
        Face {
            lit,
            glow: lit as u8 as f32,
            press: 0.0,
            hover: 0.0,
            enabled: !disabled,
        },
    );
    ui.label("");
    let state = |bound: bool, cap: bool| {
        if cap {
            KeyState::Capturing
        } else if !bound {
            KeyState::Empty
        } else if st.changed == Some(i) {
            KeyState::Changed
        } else {
            KeyState::Bound
        }
    };
    let ks = state(st.keys[i] != 0, st.capturing == Some(i));
    let cs = state(st.pads[i] != 0, st.pad_capturing == Some(i));
    let (k, c) = (
        forza::keys::key_label(st.keys[i]),
        forza::keys::pad_label(st.pads[i]),
    );
    let r1 = key_cell(ui, &k, ks, cell_w, lang);
    if i >= forza::keys::PAUSE {
        ui.label(""); // the panel's own keys (PAUSE, FLIP): the key driver reads no controller for them
        if r1.clicked() {
            st.pad_capturing = None;
            st.capturing = (st.capturing != Some(i)).then_some(i);
            st.message = None;
        }
        return;
    }
    let r2 = key_cell(ui, &c, cs, cell_w, lang);
    if r1.clicked() {
        st.pad_capturing = None;
        st.capturing = (st.capturing != Some(i)).then_some(i); // a second click cancels
        st.message = None;
    }
    if r2.clicked() {
        st.capturing = None;
        st.pad_capturing = (st.pad_capturing != Some(i)).then_some(i);
        st.message = None;
        if st.pad_capturing.is_some() && st.pad_names.is_empty() {
            st.say(Tone::Notice, "keys.msg_no_pad_yet");
        }
    }
    if r2.secondary_clicked() {
        st.pad_capturing = None;
        let label = tr!(lang, key);
        if st.pads[i] == 0 {
            st.note(
                Tone::Notice,
                tr!(lang, "keys.msg_no_control", action = label),
            );
        } else {
            st.pads[i] = 0;
            st.keys_changed = true;
            st.note(Tone::Done, tr!(lang, "keys.msg_cleared", action = label));
        }
    }
}

/// Reset keys: the first click arms it for 8 s (it wipes every key and button at once), the second resets.
fn reset_click(st: &mut PanelState, now: f64) {
    if st.keys_reset_armed <= now {
        st.keys_reset_armed = now + 8.0;
        st.say(Tone::Notice, "keys.msg_reset_armed");
        return;
    }
    st.keys_reset_armed = 0.0;
    st.keys = forza::keys::defaults(st.edition == Edition::E1718);
    st.pads = [0; forza::keys::COUNT];
    (st.capturing, st.pad_capturing, st.changed, st.keys_changed) = (None, None, None, true);
    st.say(Tone::Done, "keys.msg_defaults");
}
