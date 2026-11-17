//! Outlines as polygons: their boxes, and what is inside them.

/// An outline's (`x, y, x, y, …`) lowest x and y and highest x and y.
pub(in crate::maps) fn outline_box(p: &[f64]) -> [f64; 4] {
    let bound = |a: usize, init: f64, pick: fn(f64, f64) -> f64| p.iter().skip(a).step_by(2).fold(init, |m, v| pick(m, *v));
    [
        bound(0, f64::INFINITY, f64::min),
        bound(1, f64::INFINITY, f64::min),
        bound(0, f64::NEG_INFINITY, f64::max),
        bound(1, f64::NEG_INFINITY, f64::max),
    ]
}

/// Whether a ray from `q` towards +x crosses the edge `a`-`b` (the even-odd test).
pub(in crate::maps) fn crosses(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> bool {
    (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0]
}

/// Whether (x, y) is inside the outline `p` (`x, y, x, y, …`).
pub(in crate::maps) fn contains_point(p: &[f64], x: f64, y: f64) -> bool {
    let n = p.len() / 2;
    let at = |i: usize| [p[i % n * 2], p[i % n * 2 + 1]];
    (0..n).fold(false, |inside, i| inside ^ crosses(at(i), at(i + 1), [x, y]))
}
