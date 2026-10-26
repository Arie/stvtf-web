//! The payload track: the `path_track` chain a `team_train_watcher` sends its cart along.

use std::collections::HashSet;

use super::{Entities, Entity, Vec3};

/// The `path_track` named `name` (names are not case-sensitive).
fn node<'a>(map: &'a Entities, name: &str) -> Option<&'a Entity> {
    map.of_class("path_track").find(|e| e.get("targetname").is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// The track's segments (start and end of each), once each: from every watcher's `start_node`
/// along `target` and `altpath`, every node visited once, so loops and branches end. Without a
/// watcher there is no track, as the other `path_track`s are lifts, platforms and skybox trains.
/// A name that matches no node, or a node without an origin, ends that branch.
pub(super) fn track(map: &Entities) -> Vec<[Vec3; 2]> {
    let mut todo: Vec<String> = map.of_class("team_train_watcher").filter_map(|w| w.get("start_node")).map(str::to_lowercase).collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut segments = Vec::new();
    while let Some(name) = todo.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let Some(from) = node(map, &name) else { continue };
        let Some(a) = from.origin() else { continue };
        for next in ["target", "altpath"].into_iter().filter_map(|k| from.get(k)).map(str::to_lowercase) {
            let Some(b) = node(map, &next).and_then(Entity::origin) else { continue };
            // Two nodes pointing at each other are one segment.
            let key = if name <= next { (name.clone(), next.clone()) } else { (next.clone(), name.clone()) };
            if name != next && seen.insert(key) {
                segments.push([a, b]);
            }
            todo.push(next);
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::super::tests::swiftwater;
    use super::*;

    /// Entities from (class, keys) lists, without a BSP.
    fn entities_from(list: &[(&str, &[(&str, &str)])]) -> Entities {
        let list = list
            .iter()
            .map(|(class, keys)| {
                let keys = std::iter::once(("classname", *class)).chain(keys.iter().copied());
                Entity(keys.map(|(k, v)| (k.to_string(), v.to_string())).collect())
            })
            .collect();
        Entities { list, models: Vec::new(), points: Vec::new() }
    }

    #[test]
    fn swiftwater_track_is_the_carts_path() {
        // 178 path_tracks, 177 links; the 4 of the skybox train's loop are not the cart's.
        let Some(data) = swiftwater() else { return };
        assert_eq!(track(&Entities::read(&data).unwrap()).len(), 173);
    }

    #[test]
    fn process_has_no_track() {
        let Some(data) = super::super::tests::process() else { return };
        assert_eq!(super::super::geometry(&data).unwrap().track.len(), 0);
    }

    #[test]
    fn looping_and_branching_tracks_end() {
        // watcher -> a -> b -> c -> a (loop), b altpath -> d.
        let map = entities_from(&[
            ("team_train_watcher", &[("start_node", "A")]),
            ("path_track", &[("targetname", "a"), ("origin", "0 0 0"), ("target", "b")]),
            ("path_track", &[("targetname", "b"), ("origin", "10 0 0"), ("target", "c"), ("altpath", "D")]),
            ("path_track", &[("targetname", "c"), ("origin", "10 10 0"), ("target", "a")]),
            ("path_track", &[("targetname", "d"), ("origin", "20 0 0")]),
        ]);
        assert_eq!(track(&map).len(), 4, "a-b, b-c, c-a, b-d once each");
    }

    #[test]
    fn nodes_pointing_at_each_other_are_one_segment() {
        let map = entities_from(&[
            ("team_train_watcher", &[("start_node", "a")]),
            ("path_track", &[("targetname", "a"), ("origin", "0 0 0"), ("target", "b")]),
            ("path_track", &[("targetname", "b"), ("origin", "10 0 0"), ("target", "a")]),
        ]);
        assert_eq!(track(&map).len(), 1);
    }

    #[test]
    fn missing_nodes_end_the_branch_and_no_watcher_means_no_track() {
        let nodes: &[(&str, &[(&str, &str)])] = &[
            ("path_track", &[("targetname", "a"), ("origin", "0 0 0"), ("target", "gone")]),
            ("path_track", &[("targetname", "b"), ("target", "a")]),
        ];
        assert_eq!(track(&entities_from(nodes)).len(), 0);
        let mut with = vec![("team_train_watcher", &[("start_node", "b")][..])];
        with.extend_from_slice(nodes);
        assert_eq!(track(&entities_from(&with)).len(), 0);
    }
}
