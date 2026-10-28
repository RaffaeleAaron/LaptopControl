//! Shared, aligned controls inspired by the macOS HIG: label + precise value,
//! full-width leading-to-trailing slider, and consistent field geometry.
use eframe::egui::{self, RichText};
use std::ops::RangeInclusive;

pub fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut N,
    range: RangeInclusive<N>,
    step: f64,
    suffix: &str,
) -> egui::Response {
    ui.label(RichText::new(label).strong());
    let mut response = ui
        .horizontal(|ui| {
            let field_width = 90.;
            ui.spacing_mut().slider_width = (ui.available_width() - field_width - 16.).max(80.);
            let slider = ui.add(
                egui::Slider::new(value, range.clone())
                    .step_by(step)
                    .show_value(false)
                    .trailing_fill(true),
            );
            let field = ui.add_sized(
                [field_width, 30.],
                egui::DragValue::new(value)
                    .range(range)
                    .speed(step)
                    .suffix(suffix),
            );
            slider.union(field)
        })
        .inner;
    if response.drag_stopped() {
        response.mark_changed();
    }
    ui.add_space(4.);
    response
}
pub fn field(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([120., 32.], egui::Label::new(label));
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 32.),
            egui::Layout::left_to_right(egui::Align::Center),
            content,
        );
    });
}
pub fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .corner_radius(12.)
        .inner_margin(18.)
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.).max(100.));
            ui.label(RichText::new(title).size(18.).strong());
            ui.add_space(12.);
            content(ui);
        });
}

pub fn percent(ui: &mut egui::Ui, label: &str, value: &mut f32) -> egui::Response {
    let mut percent = *value * 100.;
    let response = slider(ui, label, &mut percent, 0. ..=100., 1., " %");
    *value = percent / 100.;
    response
}

pub fn checkbox(
    ui: &mut egui::Ui,
    value: &mut bool,
    label: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.scope(|ui| {
        let visuals = &mut ui.style_mut().visuals;
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(3);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(3);
        ui.checkbox(value, label)
    })
    .inner
}
