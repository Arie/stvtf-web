//! The floors kept: those that can be stood on, and not under shallow water.

use super::geometry::Position;
use super::material::Kind;
use super::{Floor, contains_point, outline_box};

/// How far above a floor a spawn point or pack still stands on it.
pub(super) const STANDS_ON: f64 = 64.0;

/// The least share of the floor area the fill may keep: less is seeds in a sealed pocket.
pub(super) const MIN_FILL_SHARE: f64 = 0.1;

/// Floors looked at plus outline points tested for the spawn points and packs together (real
/// maps use under a few million): a crafted map's floors after that are not looked at.
pub(super) const MAX_STAND_WORK: usize = 1 << 26;

/// Keeps the floors players get to (`keeps`) and those a spawn point or pack in `stands` is on
/// (inside the outline, 0 to `STANDS_ON` above it), and says true; when that is under
/// `MIN_FILL_SHARE` of `area` (all the floors'), keeps every floor and says false.
pub(super) fn fill(floors: &mut Vec<Floor>, keeps: impl Fn(&Floor) -> bool, stands: &[Position], area: f64) -> bool {
    let mut work_left = MAX_STAND_WORK;
    let mut stood_on = |f: &Floor| {
        for s in stands {
            let Some(left) = work_left.checked_sub(1) else { return false };
            work_left = left;
            if (0.0..=STANDS_ON).contains(&(s[2] - f.z)) {
                let Some(left) = work_left.checked_sub(f.p.len() / 2) else { return false };
                work_left = left;
                if contains_point(&f.p, s[0], s[1]) {
                    return true;
                }
            }
        }
        false
    };
    let kept: Vec<bool> = floors.iter().map(|f| keeps(f) || stood_on(f)).collect();
    let kept_area: f64 = floors.iter().zip(&kept).filter(|(_, k)| **k).map(|(f, _)| outline_area(&f.p)).sum();
    if kept_area < area * MIN_FILL_SHARE {
        tracing::info!("playable area left out: it has {:.0}% of the floor area", 100.0 * kept_area / area);
        return false;
    }
    let mut kept = kept.into_iter();
    floors.retain(|_| kept.next().unwrap_or(true));
    true
}

/// The area of an outline (`x, y, x, y, …`).
pub(super) fn outline_area(p: &[f64]) -> f64 {
    let n = p.len() / 2;
    (0..n).map(|i| p[i * 2] * p[(i + 1) % n * 2 + 1] - p[(i + 1) % n * 2] * p[i * 2 + 1]).sum::<f64>().abs() / 2.0
}

/// How far under the water a floor must be for the water to count as deep there.
pub(super) const WATER_DEPTH: f64 = 32.0;

/// How far above the water a floor still buries it.
pub(super) const WATER_BURY: f64 = 8.0;

/// Floors looked at plus outline points tested, for all the water together (real maps use
/// under 2 million): a crafted map's water after that is kept.
pub(super) const MAX_WATER_WORK: usize = 1 << 26;

/// The floors without the water that is buried or shallow: water stays when most of its sample
/// points have a non-water floor at least `WATER_DEPTH` below it, and no non-water floor
/// between that one and `WATER_BURY` above the water.
pub(super) fn without_shallow_water(floors: Vec<Floor>) -> Vec<Floor> {
    // Non-water floors with their bounding box, to skip those a point cannot be in.
    let solid: Vec<(&Floor, [f64; 4])> = floors.iter().filter(|f| f.k != Kind::Water).map(|f| (f, outline_box(&f.p))).collect();
    let mut work_left = MAX_WATER_WORK;
    // Whether the water `w` is deep, or None when the budget ran out.
    let mut deep = |w: &Floor| -> Option<bool> {
        let top = w.z + WATER_BURY;
        let b = outline_box(&w.p);
        work_left = work_left.checked_sub(solid.len())?;
        // The floors under some of the water, and not above it.
        let under: Vec<&(&Floor, [f64; 4])> =
            solid.iter().filter(|(f, s)| f.z <= top && s[0] <= b[2] && s[2] >= b[0] && s[1] <= b[3] && s[3] >= b[1]).collect();
        work_left = work_left.checked_sub(super::reach::samples_work(&w.p))?;
        let samples = super::reach::floor_samples(&w.p);
        let mut count = 0;
        for q in &samples {
            let mut highest = f64::NEG_INFINITY;
            work_left = work_left.checked_sub(under.len())?;
            for (f, s) in &under {
                if q[0] >= s[0] && q[0] <= s[2] && q[1] >= s[1] && q[1] <= s[3] && f.z > highest {
                    work_left = work_left.checked_sub(f.p.len() / 2)?;
                    if contains_point(&f.p, q[0], q[1]) {
                        highest = f.z;
                    }
                }
            }
            if highest > f64::NEG_INFINITY && highest <= w.z - WATER_DEPTH {
                count += 1;
            }
        }
        Some(count * 2 > samples.len())
    };
    let keep: Vec<bool> = floors.iter().map(|f| f.k != Kind::Water || deep(f).unwrap_or(true)).collect();
    floors.into_iter().zip(keep).filter_map(|(f, k)| k.then_some(f)).collect()
}
