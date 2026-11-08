//! The in-game panel's idle frames. The panel draws inside the game's every Present; most of those frames nothing on
//! it changed, and the egui pass (layout, then tessellation) is most of its cost. [`Idle`] says when a pass must run:
//! input arrived, egui asked for a repaint (an animation, a timer's deadline, a picture loaded on another thread), the
//! window's size or scale or the modifier keys changed, or the panel's state ([`crate::panel::PanelState::fingerprint`])
//! is not what the last pass saw. Otherwise the caller draws the last pass's meshes again: the game's picture in them
//! is a texture the caller refreshes every frame, so it stays live.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// What a pass depends on besides input and egui's repaint requests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The target's size in pixels.
    pub size: [u32; 2],
    /// Pixels per point.
    pub ppp: f32,
    pub modifiers: egui::Modifiers,
    /// The panel state's fingerprint.
    pub state: u64,
}

/// Why a pass runs: input; the panel's state changed (or nothing was drawn yet); the size, scale or modifiers
/// changed; egui asked for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wake {
    Input,
    State,
    Window,
    Repaint,
}

/// One egui context's idle gate.
pub struct Idle {
    /// When egui wants its next pass, in microseconds after `epoch` (`u64::MAX`: no request). Set by egui's repaint
    /// callback, on any thread.
    due: Arc<AtomicU64>,
    epoch: Instant,
    /// What the last pass saw; `None`: a pass must run (the first frame, after a device reset).
    seen: Option<Look>,
}

impl Idle {
    /// The gate for `ctx`: takes over its repaint callback.
    pub fn new(ctx: &egui::Context) -> Idle {
        let due = Arc::new(AtomicU64::new(u64::MAX));
        let epoch = Instant::now();
        let d = due.clone();
        ctx.set_request_repaint_callback(move |info| {
            let at = epoch
                .elapsed()
                .saturating_add(info.delay)
                .as_micros()
                .min(u128::from(u64::MAX - 1)) as u64;
            d.fetch_min(at, Ordering::AcqRel);
        });
        Idle {
            due,
            epoch,
            seen: None,
        }
    }

    /// Whether egui runs this frame, and why (`events`: input for it is waiting). When it does, the request it serves
    /// is taken and `look` kept; requests made from then on (during the pass, or from another thread) wake a later
    /// frame.
    pub fn pass(&mut self, now: Instant, events: bool, look: Look) -> Option<Wake> {
        let now = now.saturating_duration_since(self.epoch).as_micros() as u64;
        let why = if events {
            Wake::Input
        } else if self.seen.is_none_or(|s| s.state != look.state) {
            Wake::State
        } else if self.seen != Some(look) {
            Wake::Window
        } else if self.due.load(Ordering::Acquire) <= now {
            Wake::Repaint
        } else {
            return None;
        };
        self.due.store(u64::MAX, Ordering::Release);
        self.seen = Some(look);
        Some(why)
    }

    /// The next frame runs a pass whatever it sees (a device reset dropped what the painter held).
    pub fn forget(&mut self) {
        self.seen = None;
    }
}

/// The modifier keys held now as an event for the pass, only when egui does not have them yet: any event makes egui
/// ask for another pass, so one sent on every pass would keep it from ever going idle.
pub fn modifiers(ctx: &egui::Context, now: egui::Modifiers) -> Option<egui::Event> {
    (ctx.input(|i| i.modifiers) != now).then_some(egui::Event::ModifiersChanged(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{self, Page, PanelState};
    use crate::theme::{self, Edition};
    use crate::{Lang, widgets::Tone};
    use egui::{Event, Pos2, Rect, vec2};
    use std::time::Duration;

    const W: f32 = 1440.0;
    const H: f32 = 900.0;

    struct Panel {
        ctx: egui::Context,
        idle: Idle,
        st: PanelState,
        t: f64,
        passes: u32,
        /// The modifier keys held (Alt shows the deck's keycaps).
        mods: egui::Modifiers,
    }

    impl Panel {
        fn new(page: Page) -> Panel {
            let ctx = egui::Context::default();
            theme::install(&ctx, Edition::E1718, &theme::font_dir()).unwrap();
            let idle = Idle::new(&ctx);
            Panel {
                ctx,
                idle,
                st: PanelState::sample(Edition::E1718, Lang::En, page),
                t: 0.0,
                passes: 0,
                mods: egui::Modifiers::NONE,
            }
        }

        fn look(&self, size: [u32; 2]) -> Look {
            Look {
                size,
                ppp: 1.0,
                modifiers: self.mods,
                state: self.st.fingerprint(),
            }
        }

        /// One frame 1/30 s after the last, as the panel DLL runs it: `true` when egui ran.
        fn frame_at(&mut self, now: Instant, mut events: Vec<Event>, size: [u32; 2]) -> bool {
            self.t += 1.0 / 30.0;
            let look = self.look(size);
            if self.idle.pass(now, !events.is_empty(), look).is_none() {
                return false;
            }
            self.passes += 1;
            events.extend(modifiers(&self.ctx, self.mods));
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    vec2(size[0] as f32, size[1] as f32),
                )),
                time: Some(self.t),
                events,
                ..Default::default()
            };
            let st = &mut self.st;
            let mut out = self.ctx.run_ui(raw, |ui| panel::show(ui, st));
            out.textures_delta.clear();
            let _ = self.ctx.tessellate(out.shapes, out.pixels_per_point);
            true
        }

        fn frame(&mut self, events: Vec<Event>) -> bool {
            self.frame_at(Instant::now(), events, [W as u32, H as u32])
        }

        /// Frames until one skips the pass (at most 20): how many passes ran.
        fn settle(&mut self) -> u32 {
            for n in 0..20 {
                if !self.frame(vec![]) {
                    return n;
                }
            }
            panic!("egui never went idle");
        }
    }

    #[test]
    fn an_unchanged_panel_skips_the_pass() {
        let mut p = Panel::new(Page::Game);
        let n = p.settle();
        assert!(n >= 1, "the first frame runs a pass");
        for _ in 0..30 {
            assert!(!p.frame(vec![]), "nothing changed: no pass");
        }
    }

    #[test]
    fn input_forces_a_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        assert!(p.frame(vec![Event::PointerMoved(Pos2::new(5.0, 5.0))]));
        p.settle();
        assert!(!p.frame(vec![]));
    }

    #[test]
    fn a_repaint_asked_from_another_thread_forces_a_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        let ctx = p.ctx.clone();
        std::thread::spawn(move || ctx.request_repaint())
            .join()
            .unwrap();
        assert!(
            p.frame(vec![]),
            "a picture loaded off the render thread is drawn"
        );
    }

    #[test]
    fn a_hover_animation_runs_to_its_end() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        // over a page button on the bar (the middle's bottom), then still: the hover glow fades in over passes
        let on = Pos2::new(336.0, 870.0);
        assert!(p.frame(vec![Event::PointerMoved(on)]));
        assert!(
            p.frame(vec![]),
            "the glow's animation asks for the next frame"
        );
        let n = p.settle();
        assert!(n >= 1);
        assert!(!p.frame(vec![]));
    }

    /// Alt held: one pass shows the keycaps, then idle while it stays down; let go, one pass hides them.
    #[test]
    fn a_modifier_key_wakes_one_pass_each_way() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        p.mods = egui::Modifiers::ALT;
        assert!(p.frame(vec![]));
        p.settle();
        assert!(p.ctx.input(|i| i.modifiers.alt));
        assert!(!p.frame(vec![]), "still held: idle");
        p.mods = egui::Modifiers::NONE;
        assert!(p.frame(vec![]));
        p.settle();
        assert!(!p.ctx.input(|i| i.modifiers.alt));
    }

    /// Why [`modifiers`] sends only a change: an event on every pass keeps egui asking for the next one.
    #[test]
    fn any_event_asks_for_another_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        assert!(p.frame(vec![Event::ModifiersChanged(egui::Modifiers::NONE)]));
        assert!(p.frame(vec![]), "egui asks for the pass after any event");
    }

    #[test]
    fn a_state_change_forces_a_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        p.st.ping_ms += 40;
        assert!(p.frame(vec![]), "the ping meter");
        p.settle();
        p.st.tactics[0] = !p.st.tactics[0];
        assert!(p.frame(vec![]), "a tactics lamp");
        p.settle();
        p.st.board.slots.swap(0, 1);
        assert!(p.frame(vec![]), "the board read from the table");
        p.settle();
        p.st.page = Page::Keys;
        assert!(p.frame(vec![]), "a page");
    }

    #[test]
    fn a_new_size_forces_a_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        assert!(p.frame_at(Instant::now(), vec![], [1280, 800]));
        p.idle.forget();
        assert!(
            p.frame_at(Instant::now(), vec![], [1280, 800]),
            "after a device reset"
        );
    }

    /// A server notice on the message line: shown at once, gone at its deadline without any input.
    #[test]
    fn a_notice_shows_at_once_and_goes_at_its_deadline() {
        let mut p = Panel::new(Page::Game);
        p.st.message = None;
        p.settle();
        p.st.message = Some(panel::Note::long(
            Tone::Notice,
            "Server update at 23:00".into(),
        ));
        assert!(p.frame(vec![]), "the notice is drawn the frame it arrives");
        p.settle();
        assert!(p.st.message.as_ref().is_some_and(|m| m.started()));
        assert!(!p.frame(vec![]), "shown, then idle");
        // the toast's time: egui asked for a pass then; a frame after it runs one without input
        assert!(p.frame_at(
            Instant::now() + Duration::from_secs(120),
            vec![],
            [W as u32, H as u32]
        ));
    }

    /// The card dealt after a match: its countdown wakes the pass that closes it.
    #[test]
    fn a_timer_wakes_its_pass() {
        let mut p = Panel::new(Page::Game);
        p.settle();
        p.st.deal = Some(panel::Deal::new(0));
        assert!(p.frame(vec![]));
        p.settle();
        assert!(!p.frame_at(Instant::now(), vec![], [W as u32, H as u32]));
        assert!(p.frame_at(
            Instant::now() + Duration::from_secs(60),
            vec![],
            [W as u32, H as u32]
        ));
    }
}
