//! The one look of Forza-WCCF: colours, fonts and every `egui::Style` value, for the launcher, the installer and the
//! in-game panel. The direction comes from `docs/CABINET_LOOK.md`: a dark night ground, one pitch green, lit amber for
//! active orders, heavy condensed numerals. Every struct below is written out in full, so no value is egui's default.

use egui::epaint::text::{TextOptions, VariationCoords};
use egui::style::{
    HandleShape, ImeComposition, Interaction, NumericColorSpace, ScrollAnimation, ScrollStyle,
    Selection, Spacing, TextCursorStyle, WidgetVisuals, Widgets,
};
use egui::{
    Color32, Context, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId,
    FontTweak, Margin, Rangef, Shadow, Stroke, Style, TextStyle, Visuals, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---- colour tokens ----

/// Window ground, the darkest surface.
pub const NIGHT: Color32 = Color32::from_rgb(0x0B, 0x12, 0x20);
/// Below the ground: text fields, log wells, the game picture's letterbox.
pub const WELL: Color32 = Color32::from_rgb(0x06, 0x0B, 0x15);
/// Frames, cards, page panels.
pub const DEEP: Color32 = Color32::from_rgb(0x14, 0x20, 0x33);
/// Raised controls at rest.
pub const RAISED: Color32 = Color32::from_rgb(0x1C, 0x2B, 0x44);
/// Raised controls under the mouse.
pub const RAISED_HI: Color32 = Color32::from_rgb(0x25, 0x38, 0x58);
/// Hairlines and card outlines.
pub const RIM: Color32 = Color32::from_rgb(0x2B, 0x3E, 0x5E);
/// A rim under the mouse.
pub const RIM_HI: Color32 = Color32::from_rgb(0x46, 0x61, 0x8E);

pub const PITCH: Color32 = Color32::from_rgb(0x1E, 0x7A, 0x3C);
pub const PITCH_LINE: Color32 = Color32::from_rgb(0xE8, 0xF2, 0xE8);
pub const PITCH_STRIPE: Color32 = Color32::from_rgb(0x22, 0x8A, 0x43);
/// The dark pitch under a card's position map, as the fan database draws it.
pub const MAP_PITCH: Color32 = Color32::from_rgb(0x1A, 0x24, 0x30);

/// Active tactic orders, selection, "press now".
pub const AMBER: Color32 = Color32::from_rgb(0xFF, 0xB4, 0x00);
/// SHOOT, refusals, errors, STOP.
pub const RED: Color32 = Color32::from_rgb(0xD9, 0x30, 0x25);
/// Error text on the night ground (RED is too dark to read at 13 px).
pub const RED_TEXT: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);
/// KEEPER, links, focus.
pub const BLUE: Color32 = Color32::from_rgb(0x2F, 0x7D, 0xE1);
/// DATA, stat bars.
pub const CYAN: Color32 = Color32::from_rgb(0x2C, 0xC6, 0xD9);
/// Rarity gold, KEY PLAYER.
pub const GOLD: Color32 = Color32::from_rgb(0xD4, 0xA7, 0x2C);
/// Done, PLAY, "ready".
pub const GREEN: Color32 = Color32::from_rgb(0x2F, 0xB3, 0x6B);

pub const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF5, 0xFA);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8F, 0xA0, 0xBA);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5A, 0x6A, 0x85);
/// Text on a lit amber or gold face.
pub const INK: Color32 = Color32::from_rgb(0x1A, 0x12, 0x00);

/// Position chips: keepers yellow, defenders blue, midfielders green, forwards red.
pub const POS_GK: Color32 = Color32::from_rgb(0xE0, 0xA8, 0x00);
pub const POS_DF: Color32 = BLUE;
pub const POS_MF: Color32 = GREEN;
pub const POS_FW: Color32 = RED;

// ---- spacing grid: everything is a multiple of 4 ----

pub const GRID: f32 = 4.0;
pub const RADIUS: u8 = 4;
pub const RADIUS_BIG: u8 = 8;

pub use forza::Edition;

/// One design language; each edition has its own accent so a screen says which edition it belongs to.
pub trait Accent {
    fn accent(self) -> Color32;
}

impl Accent for Edition {
    /// 2010–11 is cooler (an ice cyan), 2017–18 warmer (the rarity gold).
    fn accent(self) -> Color32 {
        match self {
            Edition::E1011 => Color32::from_rgb(0x4F, 0xC3, 0xF7),
            Edition::E1718 => Color32::from_rgb(0xF0, 0xB8, 0x3C),
        }
    }
}

// ---- fonts ----

/// Noto Sans JP at 700: bold UI text, and the Japanese fallback of the display face.
pub const BOLD: &str = "bold";
/// Barlow Condensed Bold: headings, button faces, numbers. Japanese falls back to Noto Sans JP Bold.
pub const DISPLAY: &str = "display";

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// The font files, from a `fonts/` folder beside the program, else the repository's `assets/fonts/` found from the
/// working folder up (the gallery and the tests run from inside the repository).
pub fn font_dir() -> PathBuf {
    // beside the program, or in app\ beside it (a 2017-18 install's Launcher.exe)
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from));
    let mut beside = exe_dir
        .iter()
        .flat_map(|d| [d.join("fonts"), d.join("app").join("fonts")]);
    if let Some(d) = beside.find(|d| d.join(NOTO).is_file()) {
        return d;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    cwd.ancestors()
        .map(|a| a.join("assets/fonts"))
        .find(|d| d.join(NOTO).is_file())
        .unwrap_or_else(|| PathBuf::from("fonts"))
}

const NOTO: &str = "NotoSansJP-VF.ttf";
const BARLOW: &str = "BarlowCondensed-Bold.ttf";
const HACK: &str = "Hack-Regular.ttf";

/// One file read stays in memory for the program's life, shared by every weight that uses it (Noto Sans JP is ~9.6 MB).
fn read_static(dir: &Path, name: &str) -> std::io::Result<&'static [u8]> {
    // the error names the file, for the Launcher's missing-files message
    let bytes = std::fs::read(dir.join(name))
        .map_err(|e| std::io::Error::new(e.kind(), format!("{name}: {e}")))?;
    Ok(Vec::leak(bytes))
}

/// Our three families, registered explicitly (eframe's `default_fonts` stays off).
pub fn fonts(dir: &Path) -> std::io::Result<FontDefinitions> {
    Ok(fonts_from(
        read_static(dir, NOTO)?,
        read_static(dir, BARLOW)?,
        read_static(dir, HACK)?,
    ))
}

/// The same families from font bytes already in memory: a program with no `fonts/` folder beside it (the installer)
/// embeds `assets/fonts/` and calls this.
pub fn fonts_from(
    noto: &'static [u8],
    barlow: &'static [u8],
    hack: &'static [u8],
) -> FontDefinitions {
    let weight = |w: f32, scale: f32, y: f32| {
        FontData::from_static(noto).tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", w)]),
            scale,
            y_offset_factor: y,
            ..FontTweak::default()
        })
    };
    let mut defs = FontDefinitions::empty();
    // 430, a touch above Regular: the night ground makes 400 read thin at 13 px
    defs.font_data
        .insert("noto".into(), Arc::new(weight(430.0, 1.0, 0.0)));
    defs.font_data
        .insert("noto-bold".into(), Arc::new(weight(700.0, 1.0, 0.0)));
    // kana and kanji beside Barlow's tall condensed capitals: a little smaller and lower, so lines align
    defs.font_data
        .insert("noto-display".into(), Arc::new(weight(760.0, 0.86, 0.04)));
    defs.font_data
        .insert("barlow".into(), Arc::new(FontData::from_static(barlow)));
    defs.font_data
        .insert("hack".into(), Arc::new(FontData::from_static(hack)));
    let fam = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    defs.families
        .insert(FontFamily::Proportional, fam(&["noto"]));
    defs.families
        .insert(FontFamily::Name(BOLD.into()), fam(&["noto-bold"]));
    defs.families.insert(
        FontFamily::Name(DISPLAY.into()),
        fam(&["barlow", "noto-display"]),
    );
    defs.families
        .insert(FontFamily::Monospace, fam(&["hack", "noto"]));
    defs
}

/// Fonts and style on a context: what every Forza-WCCF program calls once at start.
pub fn install(ctx: &Context, edition: Edition, font_dir: &Path) -> std::io::Result<()> {
    install_fonts(ctx, edition, fonts(font_dir)?);
    Ok(())
}

/// [`install`] with the fonts already loaded.
pub fn install_fonts(ctx: &Context, edition: Edition, fonts: FontDefinitions) {
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_global_style(style(edition));
    // the card pictures: PNG files through egui's loaders
    egui_extras::install_image_loaders(ctx);
}

// ---- the style ----

fn widget(bg: Color32, rim: Stroke, fg: Color32, radius: u8, expansion: f32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: rim,
        corner_radius: CornerRadius::same(radius),
        fg_stroke: Stroke::new(1.5, fg),
        expansion,
    }
}

/// The complete style for one edition.
#[expect(deprecated)] // `clip_rect_margin` has no effect any more but is still a field
pub fn style(edition: Edition) -> Style {
    let accent = edition.accent();
    let text_styles = [
        (TextStyle::Small, body(12.0)),
        (TextStyle::Body, body(15.0)),
        (TextStyle::Button, bold(14.0)),
        (TextStyle::Heading, display(26.0)),
        (TextStyle::Monospace, mono(13.0)),
    ]
    .into();

    let visuals = Visuals {
        dark_mode: true,
        text_options: TextOptions {
            font_hinting: true,
            subpixel_binning: true,
            ..TextOptions::default()
        },
        override_text_color: None,
        weak_text_alpha: 1.0,
        weak_text_color: Some(TEXT_DIM),
        widgets: Widgets {
            noninteractive: WidgetVisuals {
                fg_stroke: Stroke::new(1.0, Color32::from_rgb(0xD8, 0xE0, 0xEC)),
                ..widget(DEEP, Stroke::new(1.0, RIM), TEXT, RADIUS, 0.0)
            },
            inactive: widget(RAISED, Stroke::new(1.0, RIM), TEXT, RADIUS, 0.0),
            hovered: widget(
                RAISED_HI,
                Stroke::new(1.0, RIM_HI),
                Color32::WHITE,
                RADIUS,
                1.0,
            ),
            active: widget(
                Color32::from_rgb(0x2E, 0x46, 0x6E),
                Stroke::new(1.5, accent),
                Color32::WHITE,
                RADIUS,
                0.0,
            ),
            open: widget(RAISED, Stroke::new(1.0, accent), TEXT, RADIUS, 0.0),
        },
        selection: Selection {
            bg_fill: accent.gamma_multiply(0.35),
            stroke: Stroke::new(1.5, accent),
        },
        ime_composition: ImeComposition {
            active_underline_stroke: Stroke::new(2.0, AMBER),
            inactive_underline_stroke: Stroke::new(1.0, TEXT_DIM),
            legacy_visuals: false,
        },
        hyperlink_color: Color32::from_rgb(0x6C, 0xAE, 0xFF),
        faint_bg_color: Color32::from_rgb(0x10, 0x1A, 0x2B),
        extreme_bg_color: WELL,
        text_edit_bg_color: Some(WELL),
        code_bg_color: WELL,
        warn_fg_color: AMBER,
        error_fg_color: RED_TEXT,
        window_corner_radius: CornerRadius::same(RADIUS_BIG),
        window_shadow: Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(150),
        },
        window_fill: DEEP,
        window_stroke: Stroke::new(1.0, RIM),
        window_highlight_topmost: false,
        menu_corner_radius: CornerRadius::same(RADIUS),
        panel_fill: NIGHT,
        popup_shadow: Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(140),
        },
        resize_corner_size: 12.0,
        text_cursor: TextCursorStyle {
            stroke: Stroke::new(2.0, AMBER),
            preview: false,
            blink: true,
            on_duration: 0.5,
            off_duration: 0.5,
        },
        clip_rect_margin: 3.0,
        button_frame: true,
        collapsing_header_frame: false,
        indent_has_left_vline: true,
        striped: true,
        slider_trailing_fill: true,
        handle_shape: HandleShape::Rect { aspect_ratio: 0.5 },
        interact_cursor: Some(CursorIcon::PointingHand),
        image_loading_spinners: true,
        numeric_color_space: NumericColorSpace::GammaByte,
        disabled_alpha: 0.45,
    };

    let spacing = Spacing {
        item_spacing: vec2(2.0 * GRID, 2.0 * GRID),
        window_margin: Margin::same(16),
        button_padding: vec2(4.0 * GRID, 2.0 * GRID),
        menu_margin: Margin::same(8),
        indent: 5.0 * GRID,
        interact_size: vec2(10.0 * GRID, 8.0 * GRID),
        slider_width: 160.0,
        slider_rail_height: 6.0,
        combo_width: 160.0,
        text_edit_width: 240.0,
        extra_text_line_spacing: 0.0,
        icon_width: 16.0,
        icon_width_inner: 8.0,
        icon_spacing: 2.0 * GRID,
        default_area_size: vec2(600.0, 400.0),
        tooltip_width: 320.0,
        menu_width: 200.0,
        menu_spacing: 2.0,
        indent_ends_with_horizontal_line: false,
        combo_height: 240.0,
        scroll: ScrollStyle {
            floating: true,
            content_margin: Margin::ZERO,
            bar_width: 8.0,
            handle_min_length: 24.0,
            bar_inner_margin: 4.0,
            bar_outer_margin: 0.0,
            floating_width: 3.0,
            floating_allocated_width: 0.0,
            foreground_color: true,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.4,
            interact_background_opacity: 0.7,
            dormant_handle_opacity: 0.35,
            active_handle_opacity: 0.7,
            interact_handle_opacity: 1.0,
            fade: egui::style::ScrollFadeStyle {
                strength: 0.6,
                size: 24.0,
            },
        },
    };

    Style {
        override_text_style: None,
        override_font_id: None,
        override_text_valign: Some(egui::Align::Center),
        text_styles,
        drag_value_text_style: TextStyle::Button,
        number_formatter: egui::style::NumberFormatter::new(
            egui::emath::format_with_decimals_in_range,
        ),
        wrap_mode: None,
        spacing,
        interaction: Interaction {
            interact_radius: 4.0,
            resize_grab_radius_side: 4.0,
            resize_grab_radius_corner: 10.0,
            show_tooltips_only_when_still: true,
            tooltip_delay: 0.6,
            tooltip_grace_time: 0.3,
            selectable_labels: false,
            multi_widget_text_select: false,
        },
        visuals,
        // CABINET_LOOK §7: 120-160 ms for hover and press, nothing longer than 300 ms
        animation_time: 0.14,
        #[cfg(debug_assertions)]
        debug: Default::default(),
        explanation_tooltips: false,
        url_in_tooltip: true,
        always_scroll_the_only_direction: true,
        scroll_animation: ScrollAnimation {
            points_per_second: 1600.0,
            duration: Rangef::new(0.08, 0.2),
        },
        compact_menu_style: true,
    }
}
