use eframe::egui::{self, Color32, Stroke};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Kde,
    Black,
    White,
}
impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Self::Kde => "Follow KDE",
            Self::Black => "Black",
            Self::White => "White",
        }
    }
}
fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
}
pub fn kde_source() -> String {
    fs::read_to_string(config_dir().join("kdeglobals")).unwrap_or_default()
}
pub fn load() -> Theme {
    fs::read(config_dir().join("raf-fans/gui.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}
fn palette(source: &str) -> BTreeMap<String, Color32> {
    let mut section = "";
    let mut map = BTreeMap::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
        } else if let Some((key, value)) = line.split_once('=') {
            let rgb: Vec<_> = value.split(',').take(3).map(str::parse::<u8>).collect();
            if let [Ok(r), Ok(g), Ok(b)] = rgb.as_slice() {
                map.insert(format!("{section}/{key}"), Color32::from_rgb(*r, *g, *b));
            }
        }
    }
    map
}
#[cfg(test)]
pub fn apply(ctx: &egui::Context, theme: Theme, source: &str) {
    apply_options(
        ctx,
        &crate::preferences::Appearance {
            theme,
            ..Default::default()
        },
        source,
    );
}
pub fn apply_options(ctx: &egui::Context, options: &crate::preferences::Appearance, source: &str) {
    let theme = options.theme;
    let p = palette(source);
    let color = |key: &str, fallback: Color32| *p.get(key).unwrap_or(&fallback);
    let kde_bg = color("Colors:Window/BackgroundNormal", Color32::from_gray(35));
    let dark = match theme {
        Theme::Kde => (u32::from(kde_bg.r()) + u32::from(kde_bg.g()) + u32::from(kde_bg.b())) < 384,
        Theme::Black => true,
        Theme::White => false,
    };
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let (bg, card, text, button) = match theme {
        Theme::Kde => (
            kde_bg,
            color("Colors:View/BackgroundNormal", v.extreme_bg_color),
            color("Colors:Window/ForegroundNormal", v.text_color()),
            color("Colors:Button/BackgroundNormal", v.widgets.inactive.bg_fill),
        ),
        Theme::Black => (
            Color32::from_gray(9),
            Color32::from_gray(20),
            Color32::from_gray(240),
            Color32::from_gray(35),
        ),
        Theme::White => (
            Color32::WHITE,
            Color32::from_gray(247),
            Color32::from_gray(25),
            Color32::from_gray(232),
        ),
    };
    let accent = color(
        "General/AccentColor",
        color(
            "Colors:Selection/BackgroundNormal",
            Color32::from_rgb(115, 100, 220),
        ),
    );
    let accent = options
        .accent
        .map(|[r, g, b]| Color32::from_rgb(r, g, b))
        .unwrap_or(accent);
    v.panel_fill = Color32::from_rgba_unmultiplied(
        bg.r(),
        bg.g(),
        bg.b(),
        (u16::from(options.opacity.clamp(30, 100)) * 255 / 100) as u8,
    );
    v.window_fill = bg;
    v.extreme_bg_color = card;
    v.faint_bg_color = card;
    v.override_text_color = Some(text);
    v.widgets.noninteractive.bg_fill = card;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1., text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(
        1.,
        if dark {
            Color32::from_gray(70)
        } else {
            Color32::from_gray(205)
        },
    );
    v.widgets.inactive.bg_fill = button;
    v.widgets.inactive.weak_bg_fill = button;
    v.widgets.inactive.fg_stroke = Stroke::new(1., text);
    v.widgets.hovered.bg_fill = accent.gamma_multiply(0.65);
    v.widgets.hovered.fg_stroke = Stroke::new(1., text);
    v.selection.bg_fill = accent;
    v.selection.stroke = Stroke::new(1., Color32::WHITE);
    v.hyperlink_color = accent;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.45 };
    for widget in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(8);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(12., 10.);
        style.spacing.button_padding = egui::vec2(12., 7.);
        style.spacing.interact_size = egui::vec2(80., 32.);
        style.spacing.combo_height = 280.;
        style.animation_time = 0.08;
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_kde_palette() {
        let p = palette(
            "[Colors:Window]\nBackgroundNormal=12,24,36\n[General]\nAccentColor=120,80,220\n",
        );
        assert_eq!(
            p["Colors:Window/BackgroundNormal"],
            Color32::from_rgb(12, 24, 36)
        );
        assert_eq!(p["General/AccentColor"], Color32::from_rgb(120, 80, 220));
    }
}
#[cfg(test)]
mod switching_tests {
    use super::*;
    #[test]
    fn switching_and_following_kde_changes_the_palette() {
        let ctx = egui::Context::default();
        let source="[Colors:Window]\nBackgroundNormal=45,46,47\nForegroundNormal=240,240,240\n[General]\nAccentColor=120,80,220\n";
        apply(&ctx, Theme::Black, source);
        assert_eq!(
            ctx.global_style().visuals.panel_fill.to_opaque(),
            Color32::from_gray(9)
        );
        apply(&ctx, Theme::White, source);
        assert_eq!(
            ctx.global_style().visuals.panel_fill.to_opaque(),
            Color32::WHITE
        );
        apply(&ctx, Theme::Kde, source);
        assert_eq!(
            ctx.global_style().visuals.panel_fill.to_opaque(),
            Color32::from_rgb(45, 46, 47)
        );
        assert_eq!(
            ctx.global_style().visuals.selection.bg_fill,
            Color32::from_rgb(120, 80, 220)
        );
    }
}
