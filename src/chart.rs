use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use raf_fans::{curve::move_point, FanConfig};
pub fn editor(
    ui: &mut egui::Ui,
    config: &mut FanConfig,
    selected: &mut usize,
    current: Option<f32>,
) -> bool {
    let mut changed = false;
    ui.label(
        "Drag a point to shape the curve. Click empty space to add; right-click a point to remove.",
    );
    let (outer, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 200.), egui::Sense::hover());
    let plot = Rect::from_min_max(
        outer.min + Vec2::new(42., 14.),
        outer.max - Vec2::new(18., 30.),
    );
    let position = |v: [f32; 2]| {
        Pos2::new(
            egui::lerp(plot.x_range(), (v[0] - 20.) / 80.),
            egui::lerp(plot.y_range(), 1. - v[1] / 100.),
        )
    };
    let value = |p: Pos2| {
        [
            (20. + (p.x - plot.left()) / plot.width() * 80.).clamp(20., 90.),
            ((plot.bottom() - p.y) / plot.height() * 100.).clamp(0., 100.),
        ]
    };
    let accent = ui.visuals().selection.bg_fill;
    let text = ui.visuals().text_color();
    let grid = ui.visuals().widgets.noninteractive.bg_stroke;
    let response = ui.interact(plot, ui.id().with("curve-canvas"), egui::Sense::click());
    let painter = ui.painter_at(outer);
    painter.rect_filled(plot, 4., ui.visuals().extreme_bg_color);
    for t in (20..=100).step_by(if plot.width() < 450. { 20 } else { 10 }) {
        let x = position([t as f32, 0.]).x;
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
        painter.text(
            Pos2::new(x, plot.bottom() + 8.),
            egui::Align2::CENTER_TOP,
            format!("{t}°"),
            egui::FontId::proportional(11.),
            text,
        );
    }
    for output in (0..=100).step_by(25) {
        let y = position([20., output as f32]).y;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            grid,
        );
        painter.text(
            Pos2::new(plot.left() - 8., y),
            egui::Align2::RIGHT_CENTER,
            format!("{output}%"),
            egui::FontId::proportional(11.),
            text,
        );
    }
    if let Some(t) = current {
        let x = position([t.clamp(20., 100.), 0.]).x;
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.5, text.gamma_multiply(0.45)),
        );
    }
    let mut remove = None;
    let mut point_hit = false;
    for index in 0..config.curve.len() {
        let point = position(config.curve[index]);
        let dot = ui.interact(
            Rect::from_center_size(point, Vec2::splat(24.)),
            ui.id().with(("curve-point", index)),
            egui::Sense::click_and_drag(),
        );
        point_hit |= dot.hovered() || dot.dragged() || dot.clicked();
        if dot.clicked() || dot.drag_started() {
            *selected = index;
        }
        if dot.dragged() {
            if let Some(pos) = dot.interact_pointer_pos() {
                move_point(&mut config.curve, index, value(pos));
                changed = true;
            }
        }
        if dot.secondary_clicked() && config.curve.len() > 2 && index + 1 < config.curve.len() {
            remove = Some(index);
        }
        dot.on_hover_text(format!(
            "{:.0}°C → {:.0}%",
            config.curve[index][0], config.curve[index][1]
        ));
    }
    if let Some(index) = remove {
        config.curve.remove(index);
        *selected = (*selected).min(config.curve.len() - 1);
        changed = true;
    }
    if response.clicked() && !point_hit && config.curve.len() < 12 {
        if let Some(pos) = response.interact_pointer_pos() {
            let p = value(pos);
            let index = config.curve.partition_point(|v| v[0] < p[0]);
            let min = if index == 0 {
                20.
            } else {
                config.curve[index - 1][0] + 1.
            };
            let max = if index == config.curve.len() {
                90.
            } else {
                config.curve[index][0] - 1.
            };
            if min <= max && index < config.curve.len() {
                config.curve.insert(index, [p[0].clamp(min, max), p[1]]);
                move_point(&mut config.curve, index, p);
                *selected = index;
                changed = true;
            }
        }
    }
    let points: Vec<_> = config.curve.iter().copied().map(position).collect();
    let mut extended = vec![position([20., config.curve[0][1]])];
    extended.extend(points.iter());
    extended.push(position([100., 100.]));
    for pair in extended.windows(2) {
        let mut mesh = egui::Mesh::default();
        let base = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 32);
        for p in [
            pair[0],
            pair[1],
            Pos2::new(pair[1].x, plot.bottom()),
            Pos2::new(pair[0].x, plot.bottom()),
        ] {
            mesh.colored_vertex(p, base);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
    painter.add(egui::Shape::line(extended, Stroke::new(2.5, accent)));
    for (i, p) in points.iter().enumerate() {
        painter.circle_filled(*p, if i == *selected { 7. } else { 5. }, accent);
        painter.circle_stroke(
            *p,
            if i == *selected { 10. } else { 7. },
            Stroke::new(1.5, text),
        );
    }
    *selected = (*selected).min(config.curve.len() - 1);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Point {}", *selected + 1)).strong());
        let mut p = config.curve[*selected];
        let x = ui
            .add(
                egui::DragValue::new(&mut p[0])
                    .range(20. ..=90.)
                    .suffix(" °C"),
            )
            .changed();
        let y = ui
            .add_enabled(
                *selected + 1 < config.curve.len(),
                egui::DragValue::new(&mut p[1])
                    .range(0. ..=100.)
                    .suffix(" %"),
            )
            .changed();
        if x || y {
            move_point(&mut config.curve, *selected, p);
            changed = true;
        }
        if ui.button("Reset curve").clicked() {
            config.curve = FanConfig::default().curve;
            *selected = 0;
            changed = true;
        }
    });
    ui.small("The last point stays at 100%. Positive requests below 25% use the EC minimum; 0% requests fan-off.");
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        c: &mut FanConfig,
        selected: &mut usize,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default_margins().show(ctx, |ui| {
                    editor(ui, c, selected, None);
                });
            },
        )
    }
    #[test]
    fn pointer_drag_changes_curve_point() {
        let ctx = egui::Context::default();
        let mut c = FanConfig::default();
        let mut selected = 0;
        let out = frame(&ctx, &mut c, &mut selected, 0., vec![]);
        let dots: Vec<_> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Circle(circle) if circle.fill != Color32::TRANSPARENT => {
                    Some(circle.center)
                }
                _ => None,
            })
            .collect();
        let from = dots[1];
        let xscale = (dots[1].x - dots[0].x) / 15.;
        let yscale = (dots[0].y - dots[1].y) / 30.;
        let to = from + Vec2::new(5. * xscale, -10. * yscale);
        frame(
            &ctx,
            &mut c,
            &mut selected,
            0.1,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            &ctx,
            &mut c,
            &mut selected,
            0.2,
            vec![egui::Event::PointerMoved(to)],
        );
        frame(
            &ctx,
            &mut c,
            &mut selected,
            0.3,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(selected, 1);
        assert_eq!(c.curve[1], [55., 40.]);
        assert!(c.validate().is_ok());
    }
}
