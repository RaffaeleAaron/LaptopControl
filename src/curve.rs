/// Constrain a point to its neighbours so dragging cannot make an invalid curve.
pub fn move_point(points: &mut [[f32; 2]], index: usize, value: [f32; 2]) {
    if index >= points.len() || value.iter().any(|v| !v.is_finite()) {
        return;
    }
    let min_x = if index == 0 {
        20.
    } else {
        points[index - 1][0] + 1.
    };
    let max_x = if index + 1 == points.len() {
        90.
    } else {
        points[index + 1][0] - 1.
    };
    let min_y = if index == 0 { 0. } else { points[index - 1][1] };
    let max_y = if index + 1 == points.len() {
        100.
    } else {
        points[index + 1][1]
    };
    points[index] = [
        value[0].round().clamp(min_x, max_x),
        if index + 1 == points.len() {
            100.
        } else {
            value[1].round().clamp(min_y, max_y)
        },
    ];
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_preserves_order_and_allows_zero() {
        let mut c = crate::FanConfig::default();
        move_point(&mut c.curve, 0, [10., -5.]);
        assert_eq!(c.curve[0], [20., 0.]);
        move_point(&mut c.curve, 1, [89., 99.]);
        assert_eq!(c.curve[1], [64., 50.]);
        move_point(&mut c.curve, 4, [70., 30.]);
        assert_eq!(c.curve[4], [76., 100.]);
        assert!(c.validate().is_ok());
    }
}
