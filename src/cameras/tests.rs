use super::*;

fn placed(name: &str, x: i32) -> Change {
    Change { id: None, name: name.into(), pos: Some([x, 0, 1500]), aim: Some([0, 0, 400]), going: None, still: false, remove: false }
}

fn names(c: &Cameras, map: &str) -> (String, Vec<String>) {
    let on = c.on(map);
    let names = on["cameras"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
    (on["map"].as_str().unwrap().to_string(), names)
}

#[test]
fn a_maps_version_is_not_part_of_its_stem() {
    for (map, is) in [
        ("cp_gullywash_f9", "cp_gullywash"),
        ("cp_gullywash_final1", "cp_gullywash"),
        ("koth_product_final", "koth_product"),
        ("cp_granary_pro_rc8", "cp_granary_pro"),
        ("koth_clearcut_b15d", "koth_clearcut"),
        ("cp_sunshine", "cp_sunshine"),
        ("pl_upward_f12", "pl_upward"),
        ("koth_bagel_rc10", "koth_bagel"),
        ("cp_process_f12_fix", "cp_process"),
        ("ctf_2fort", "ctf_2fort"),
        ("cp_5gorge", "cp_5gorge"),
        ("koth_b", "koth_b"),
    ] {
        assert_eq!(stem(map), is, "{map}");
    }
}

#[test]
fn cameras_are_kept_by_map_and_a_new_version_of_a_map_starts_with_the_last_ones() {
    let dir = tempfile::tempdir().unwrap();
    let c = Cameras::new(Some(dir.path()), None);
    c.change("cp_gullywash_f9", &placed("Mid", 100)).unwrap();
    c.change("cp_gullywash_f9", &placed("", 200)).unwrap();
    c.change("cp_process_f12", &placed("Other", 1)).unwrap();
    assert_eq!(names(&c, "cp_gullywash_f9"), ("cp_gullywash_f9".into(), vec!["Mid".into(), "Camera 2".into()]));
    assert_eq!(names(&c, "cp_gullywash_final1").0, "cp_gullywash_f9", "the same map by another name");
    assert_eq!(names(&c, "cp_snakewater_final1"), ("cp_snakewater_final1".into(), vec![]));
    // Once changed there, the map has its own; those of the older version stay as they were.
    let mid = c.on("cp_gullywash_final1")["cameras"][0]["id"].as_u64().unwrap() as u32;
    c.change(
        "cp_gullywash_final1",
        &Change { id: Some(mid), name: "Middle".into(), pos: None, aim: None, going: None, still: false, remove: false },
    )
    .unwrap();
    c.change(
        "cp_gullywash_final1",
        &Change { id: Some(mid + 1), name: String::new(), pos: None, aim: None, going: None, still: false, remove: true },
    )
    .unwrap();
    assert_eq!(names(&c, "cp_gullywash_final1"), ("cp_gullywash_final1".into(), vec!["Middle".into()]));
    assert_eq!(names(&c, "cp_gullywash_f9").1, ["Mid", "Camera 2"]);
    assert_eq!(names(&c, "cp_gullywash_f10").0, "cp_gullywash_final1", "the latest changed");
    assert_eq!(names(&Cameras::new(Some(dir.path()), None), "cp_gullywash_final1").1, ["Middle"], "read back");
}

#[test]
fn a_camera_off_the_map_or_named_too_long_is_refused() {
    let c = Cameras::default();
    assert!(c.change("../etc", &placed("a", 1)).is_err());
    assert!(c.change("cp_a", &placed(&"n".repeat(MAX_NAME + 1), 1)).is_err());
    assert!(c.change("cp_a", &placed("a", 99_999)).is_err());
    assert!(
        c.change("cp_a", &Change { id: Some(4), name: "a".into(), pos: None, aim: None, going: None, still: false, remove: false })
            .is_err()
    );
    assert!(
        c.change("cp_a", &Change { id: None, name: "a".into(), pos: None, aim: None, going: None, still: false, remove: false }).is_err()
    );
    for n in 0..MAX_ON_MAP {
        c.change("cp_b", &placed("", n as i32)).unwrap();
    }
    assert!(c.change("cp_b", &placed("", 1)).is_err());
}

#[test]
fn a_region_passes_changes_on_to_the_home_and_has_what_the_home_has() {
    let home = Cameras::default();
    let (h1, h2) = (home.clone(), home.clone());
    let region = Cameras::asking(
        None,
        Some(Home { since: Box::new(move |v| Ok(h1.since(v))), change: Box::new(move |map, change| h2.change(map, change)) }),
    );
    home.change("cp_a", &placed("Theirs", 1)).unwrap();
    region.change("cp_a", &placed("Ours", 2)).unwrap();
    assert_eq!(names(&home, "cp_a").1, ["Theirs", "Ours"]);
    assert_eq!(names(&region, "cp_a").1, ["Theirs", "Ours"]);
    assert!(region.change("cp_a", &placed("a", 99_999)).is_err());
    home.change("cp_a", &placed("Later", 3)).unwrap();
    region.sync().unwrap();
    assert_eq!(names(&region, "cp_a").1.len(), 3);
    assert_eq!(home.since(lock(&region.0.kept).version), None);
}

#[test]
fn a_camera_keeps_the_move_it_was_given_until_told_it_stays() {
    let mut k = Kept::default();
    let change = |text: &str| serde_json::from_str::<Change>(text).unwrap();
    let arc = r#"{"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":8,"way":"arc","turn":1.5}"#;
    k.change("cp_a", &change(&format!(r#"{{"name":"Orbit","pos":[900,0,900],"aim":[0,0,300],"move":{arc}}}"#))).unwrap();
    let said = |k: &Kept| serde_json::to_value(k.by_map()["cp_a"]).unwrap()[0]["move"].clone();
    assert_eq!(said(&k), serde_json::json!({"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":8.0,"way":"arc","turn":1.5}));
    // Placed anew or named anew, it keeps it.
    k.change("cp_a", &change(r#"{"id":1,"name":"Round","pos":[800,0,900],"aim":[0,0,300]}"#)).unwrap();
    assert_eq!(said(&k)["way"], "arc");
    // A flight's moments, each the seconds in, where it is and what it looks at.
    let flight = r#"{"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":2,"way":"straight","took":2,"even":0.25,
        "path":[[0,900,0,900,0,0,300],[2,0,900,900,0,0,300]]}"#;
    k.change("cp_a", &change(&format!(r#"{{"id":1,"move":{flight}}}"#))).unwrap();
    assert_eq!(said(&k)["path"][1][2], 900.0);
    for bad in [r#""secs":0"#, r#""secs":500"#, r#""turn":99"#, r#""path":[]"#, r#""path":[[1,0,0,1,0,0,0],[0,0,0,1,0,0,0]]"#] {
        let odd = format!(
            r#"{{"id":1,"move":{{"end":{{"pos":[0,900,900],"aim":[0,0,300]}},"way":"arc",{}{bad}}}}}"#,
            if bad.contains("secs") { "" } else { r#""secs":2,"# }
        );
        assert!(k.change("cp_a", &change(&odd)).is_err(), "{bad}");
    }
    k.change("cp_a", &change(r#"{"id":1,"still":true}"#)).unwrap();
    assert!(said(&k).is_null());
    // The flights on a map go to every page, so their rows are capped (`MAX_ROWS_ON_MAP`).
    let long: Vec<String> = (0..1800).map(|n| format!("[{},900,0,900,0,0,300]", f64::from(n) / 30.0)).collect();
    let flown = format!(r#""move":{{"end":{{"pos":[0,900,900],"aim":[0,0,300]}},"secs":60,"way":"straight","path":[{}]}}"#, long.join(","));
    for n in 0..3 {
        k.change("cp_a", &change(&format!(r#"{{"name":"f{n}","pos":[900,0,900],"aim":[0,0,300],{flown}}}"#))).unwrap();
    }
    let more = k.change("cp_a", &change(&format!(r#"{{"name":"f3","pos":[900,0,900],"aim":[0,0,300],{flown}}}"#)));
    assert_eq!(more.unwrap_err().to_string(), "Too many flights on this map.");
    // One flown again is counted once.
    k.change("cp_a", &change(&format!(r#"{{"id":2,{flown}}}"#))).unwrap();
}

#[test]
fn a_spot_is_a_relays_camera_that_stays_or_goes_along_a_path_given() {
    let spot = Spot { pos: [1000, 0, 1400], aim: [0, 1000, 400] };
    let stvtf::relay::Camera::Along(still) = Shot::new(spot, None).unwrap().for_tf2() else { panic!() };
    let at = stvtf::relay::Step { secs: 0.0, pos: [1000.0, 0.0, 1400.0], aim: [0.0, 1000.0, 400.0] };
    assert_eq!(still.steps, [at]);
    let rows = [[0.0, 1000.0, 0.0, 1400.0, 0.0, 1000.0, 400.0], [2.5, 900.5, 0.0, 1400.0, 0.0, 1000.0, 400.0]];
    let moving = Shot::new(spot, Some(&rows)).unwrap();
    assert_eq!((moving.path.steps.len(), moving.path.steps[1].secs, moving.path.steps[1].pos[0]), (2, 2.5, 900.5));
    assert_ne!(moving.path.id, 0);
    // Each gets a new id, even two made in the same millisecond.
    let ids: Vec<u64> = (0..50).map(|_| Shot::new(spot, None).unwrap().path.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]) && ids[0] > moving.path.id);
    assert!(!Spot { pos: [1, 1, 1], aim: [1, 1, 1] }.fits());
    // Out of order, too long, off the map, looking at itself, not a number, none.
    let bad: [&[Row]; 6] = [
        &[rows[1], rows[0]],
        &[rows[0], [121.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]],
        &[[0.0, 99_999.0, 0.0, 0.0, 1.0, 0.0, 0.0]],
        &[[0.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0]],
        &[[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]],
        &[],
    ];
    for rows in bad {
        assert!(Shot::new(spot, Some(rows)).is_err(), "{rows:?}");
    }
}
