//! A match's map loads, counted from 0: a new one starts at a snapshot after a server line naming
//! another map than the latest load's, or at one whose tick is lower than the latest load's last
//! (the server's ticks started over: the same map loaded again). A server line naming the latest
//! load's map, its ticks going on after, is no new load (a reconnect, a repeated announcement).

use serde::{Deserialize, Serialize};

/// Which load of the match a snapshot is of: one rule for the recording and the demos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Loads {
    n: usize,
    /// The latest load's map, and the map the latest server line named.
    map: Option<String>,
    named: Option<String>,
    /// The latest load's last tick.
    last_tick: Option<u32>,
    /// A load began: the next snapshot starts load `n`, not `n + 1`, until one has.
    begun: bool,
}

impl Loads {
    /// Going on from load `n` of `map`, begun already, its latest snapshot at `last_tick`: the next
    /// snapshot is of `n` still, unless the rule says another.
    pub(crate) fn from(n: usize, map: Option<String>, last_tick: Option<u32>) -> Loads {
        Loads { n, named: map.clone(), map, last_tick, begun: true }
    }

    /// A server line, naming `map`.
    pub(crate) fn server(&mut self, map: &str) {
        self.named = Some(map.to_string());
    }

    /// Whether a snapshot at `tick` would start a load.
    pub(crate) fn starts(&self, tick: u32) -> bool {
        !self.begun || self.named != self.map || self.last_tick.is_some_and(|last| tick < last)
    }

    /// The load a snapshot at `tick` is of, and whether it starts here.
    pub(crate) fn snapshot(&mut self, tick: u32) -> (usize, bool) {
        let starts = self.starts(tick);
        if starts {
            self.n += usize::from(self.begun);
            (self.begun, self.map) = (true, self.named.clone());
        }
        self.last_tick = Some(tick);
        (self.n, starts)
    }

    /// The load a demo whose first frame is at `first_tick` of `map` will be of (nothing changes).
    #[cfg_attr(not(test), expect(dead_code, reason = "for the demos' recorder"))]
    pub(crate) fn would_be(&self, map: &str, first_tick: u32) -> usize {
        let new = self.map.as_deref() != Some(map) || self.last_tick.is_some_and(|last| first_tick < last);
        self.n + usize::from(self.begun && new)
    }

    pub(crate) fn latest(&self) -> usize {
        self.n
    }

    /// The latest load's map, once one began.
    pub(crate) fn map(&self) -> Option<&str> {
        self.map.as_deref()
    }

    /// The latest load's last tick.
    pub(crate) fn last_tick(&self) -> Option<u32> {
        self.last_tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(map: &str) -> Loads {
        let mut loads = Loads::default();
        loads.server(map);
        loads
    }

    #[test]
    fn the_first_snapshot_starts_load_0() {
        let mut loads = on("cp_a");
        assert_eq!(loads.latest(), 0);
        assert_eq!(loads.snapshot(500), (0, true));
        assert_eq!(loads.snapshot(550), (0, false));
        assert_eq!(loads.map(), Some("cp_a"));
    }

    #[test]
    fn another_map_named_starts_a_load_at_its_first_snapshot() {
        let mut loads = on("cp_a");
        loads.snapshot(9000);
        loads.server("cp_b");
        assert_eq!(loads.latest(), 0, "not at the server line");
        assert_eq!(loads.snapshot(9100), (1, true), "its ticks need not start over");
        assert_eq!(loads.map(), Some("cp_b"));
        assert_eq!(loads.snapshot(9150), (1, false));
    }

    #[test]
    fn ticks_starting_over_are_the_same_map_again() {
        let mut loads = on("cp_a");
        loads.snapshot(9000);
        loads.server("cp_a");
        assert_eq!(loads.snapshot(68), (1, true));
        assert_eq!(loads.snapshot(244), (1, false), "a frozen frame, then play: one load");
        assert_eq!(loads.snapshot(100), (2, true), "with no server line too");
    }

    #[test]
    fn a_reconnect_is_no_new_load() {
        let mut loads = on("cp_a");
        loads.snapshot(9000);
        loads.server("cp_a");
        loads.server("cp_a");
        assert_eq!(loads.snapshot(9300), (0, false));
    }

    #[test]
    fn would_be_is_what_the_snapshot_after_says() {
        let fresh = on("cp_a");
        assert_eq!(fresh.would_be("cp_a", 500), 0);
        for (map, tick) in [("cp_a", 9100), ("cp_a", 68), ("cp_b", 9100), ("cp_b", 68)] {
            let mut loads = on("cp_a");
            loads.snapshot(9000);
            let would = loads.would_be(map, tick);
            loads.server(map);
            assert_eq!(loads.would_be(map, tick), would, "a server line first changes nothing");
            assert_eq!(loads.snapshot(tick).0, would, "{map} at {tick}");
            assert_eq!(loads.would_be(map, tick + 1), would, "and after it, the same load");
        }
    }

    #[test]
    fn from_goes_on_from_a_load_begun() {
        let mut loads = Loads::from(3, Some("cp_a".into()), Some(9000));
        assert_eq!((loads.latest(), loads.would_be("cp_a", 9100)), (3, 3));
        assert_eq!(loads.snapshot(9100), (3, false));
        assert_eq!(loads.snapshot(50), (4, true));
        let mut unticked = Loads::from(2, Some("cp_a".into()), None);
        assert_eq!(unticked.snapshot(50), (2, false), "its ticks not known: whatever comes goes on");
        unticked.server("cp_b");
        assert_eq!(unticked.snapshot(60), (3, true));
    }
}
