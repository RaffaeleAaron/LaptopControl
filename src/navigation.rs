use eframe::egui::{self, Pos2, Stroke, Vec2};
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Fans,
    Camera,
    Monitors,
    Settings,
}
pub fn button(ui: &mut egui::Ui, page: Page, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.), egui::Sense::click());
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, 6., fill);
    let color = ui.visuals().text_color();
    let c = rect.left_center()
        + Vec2::new(
            if rect.width() > 90. {
                25.
            } else {
                rect.width() / 2.
            },
            0.,
        );
    let stroke = Stroke::new(1.8, color);
    match page {
        Page::Fans => {
            p.circle_stroke(c, 16., stroke);
            p.circle_filled(c, 3., color);
            for i in 0..3 {
                let a = i as f32 * std::f32::consts::TAU / 3.;
                let point = |r: f32, angle: f32| c + Vec2::angled(a + angle) * r;
                p.add(egui::Shape::convex_polygon(
                    vec![
                        point(5., 0.),
                        point(14., 0.15),
                        point(14., 0.8),
                        point(8., 1.1),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Page::Camera => {
            p.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(32., 21.)),
                4.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(c, 6., stroke);
            p.circle_filled(c + Vec2::new(11., -5.), 1.5, color);
            p.line_segment([c + Vec2::new(0., 11.), c + Vec2::new(0., 17.)], stroke);
            p.line_segment([c + Vec2::new(-10., 17.), c + Vec2::new(10., 17.)], stroke);
        }
        Page::Monitors => {
            p.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(32., 25.)),
                4.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::line(
                vec![
                    c + Vec2::new(-12., 4.),
                    c + Vec2::new(-6., 4.),
                    c + Vec2::new(-2., -7.),
                    c + Vec2::new(3., 8.),
                    c + Vec2::new(7., 0.),
                    c + Vec2::new(12., 0.),
                ],
                stroke,
            ));
        }
        Page::Settings => {
            p.circle_stroke(c, 8., stroke);
            p.circle_stroke(c, 3., stroke);
            for i in 0..8 {
                let d = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.);
                p.line_segment([c + d * 10., c + d * 15.], Stroke::new(3., color));
            }
        }
    }
    let label = match page {
        Page::Fans => "Fans",
        Page::Camera => "Webcam",
        Page::Monitors => "Monitors",
        Page::Settings => "Settings",
    };
    if rect.width() > 90. {
        p.text(
            Pos2::new(rect.left() + 53., rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(16.),
            color,
        );
    }
    response.on_hover_text(label)
}

pub fn collapse_button(ui: &mut egui::Ui, collapsed: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.), egui::Sense::click());
    let t = ui
        .ctx()
        .animate_bool_with_time(ui.id().with("navigation-toggle"), !collapsed, 0.16);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 8., ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let center = rect.center();
    let lerp = |a: Vec2, b: Vec2| center + a * (1. - t) + b * t;
    let stroke = Stroke::new(2., ui.visuals().text_color());
    for (a, b, c, d) in [
        (
            Vec2::new(-8., -6.),
            Vec2::new(8., -6.),
            Vec2::new(-1., -7.),
            Vec2::new(-8., 0.),
        ),
        (
            Vec2::new(-8., 0.),
            Vec2::new(8., 0.),
            Vec2::new(-8., 0.),
            Vec2::new(8., 0.),
        ),
        (
            Vec2::new(-8., 6.),
            Vec2::new(8., 6.),
            Vec2::new(-1., 7.),
            Vec2::new(-8., 0.),
        ),
    ] {
        ui.painter().line_segment([lerp(a, c), lerp(b, d)], stroke);
    }
    response
}
