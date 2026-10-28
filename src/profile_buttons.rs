use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use raf_fans::Profile;
/// Original vector icons: microchip, quiet speaker, and rocket.
fn icon(ui: &egui::Ui, profile: Profile, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let p = |x: f32, y: f32| rect.min + Vec2::new(x, y);
    let stroke = Stroke::new(1.5, color);
    match profile {
        Profile::Firmware => {
            painter.rect_stroke(
                Rect::from_min_max(p(4., 4.), p(16., 16.)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(7., 7.), p(13., 13.)),
                0.,
                stroke,
                egui::StrokeKind::Inside,
            );
            for v in [6., 10., 14.] {
                for pair in [
                    [p(v, 0.), p(v, 4.)],
                    [p(v, 16.), p(v, 20.)],
                    [p(0., v), p(4., v)],
                    [p(16., v), p(20., v)],
                ] {
                    painter.line_segment(pair, stroke);
                }
            }
        }
        Profile::Office => {
            painter.add(egui::Shape::closed_line(
                vec![
                    p(2., 8.),
                    p(6., 8.),
                    p(11., 4.),
                    p(11., 16.),
                    p(6., 12.),
                    p(2., 12.),
                ],
                stroke,
            ));
            painter.line_segment([p(14., 8.), p(18., 12.)], stroke);
            painter.line_segment([p(14., 12.), p(18., 8.)], stroke);
        }
        Profile::Gaming => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    p(8., 14.),
                    p(6., 9.),
                    p(10., 3.),
                    p(18., 1.),
                    p(16., 9.),
                    p(11., 14.),
                ],
                color,
                Stroke::NONE,
            ));
            painter.circle_filled(p(13., 6.), 2., ui.visuals().panel_fill.to_opaque());
            painter.add(egui::Shape::convex_polygon(
                vec![p(6., 9.), p(1., 11.), p(1., 16.), p(8., 14.)],
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![p(11., 14.), p(10., 20.), p(15., 19.), p(16., 10.)],
                color,
                Stroke::NONE,
            ));
            painter.line_segment([p(6., 15.), p(3., 18.)], Stroke::new(2., color));
        }
    }
}
pub fn button(ui: &mut egui::Ui, profile: Profile, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(112., 38.), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), profile.label())
    });
    let accent = ui.visuals().selection.bg_fill;
    let fill = if selected {
        accent
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        ui.visuals().widgets.inactive.weak_bg_fill
    };
    let color = if selected {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    ui.painter().rect_filled(rect, 5., fill);
    icon(
        ui,
        profile,
        Rect::from_min_size(
            Pos2::new(rect.left() + 10., rect.center().y - 10.),
            Vec2::splat(20.),
        ),
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 38., rect.center().y),
        egui::Align2::LEFT_CENTER,
        profile.label(),
        egui::FontId::proportional(14.),
        color,
    );
    response
}
