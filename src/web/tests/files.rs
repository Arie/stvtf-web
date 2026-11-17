//! The page, its modules, and the fixed files, pictures and maps served with it.

use super::*;

fn request(send: &[u8]) -> (Result<()>, String) {
    request_to(&app(Matches::One(SharedHub::default())), send)
}

#[test]
fn other_sites_load_the_viewer_from_a_fixed_address() {
    let loader = request(b"GET /embed/v1/stv-map.js HTTP/1.1\r\n\r\n").1;
    assert!(loader.contains(&format!("\"../../app/{}/stv-map.js\"", app_version())) && loader.contains("max-age=300"), "{loader}");
    let module = request(format!("GET /app/{}/stv-map.js HTTP/1.1\r\n\r\n", app_version()).as_bytes()).1;
    assert!(module.contains("customElements.define") && module.contains("Access-Control-Allow-Origin: *"), "{module}");
}

#[test]
fn other_sites_get_the_kill_icons_with_crits_under_a_version() {
    let css = request(b"GET /killicons/v1/killicons.css HTTP/1.1\r\n\r\n").1;
    assert!(css.starts_with("HTTP/1.1 200") && css.contains(".killicon-quake_rl") && css.contains(".killicon.crit"), "{css}");
}

#[test]
fn serves_the_page_with_nosniff() {
    let (result, response) = request(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    result.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:.100}");
    assert!(response.contains("\r\nX-Content-Type-Options: nosniff\r\n"));
}

#[test]
fn other_sites_read_what_is_public_and_frame_no_page_that_acts_for_someone() {
    let many = app(Matches::Many(Sessions::new(false, 1, 20.0)));
    let get = |path: &str| request_to(&many, format!("GET {path} HTTP/1.1\r\nOrigin: https://other.example\r\n\r\n").as_bytes()).1;
    let front = get("/");
    assert!(front.contains("\r\nX-Frame-Options: DENY\r\n") && front.contains("\r\nAccess-Control-Allow-Origin: *\r\n"), "{front}");
    assert!(get("/mumble").contains("X-Frame-Options: DENY"));
    // A match's page is for other sites' frames (the embed).
    let page = get(&format!("/app/{}/main.js", app_version()));
    assert!(page.starts_with("HTTP/1.1 200") && !page.contains("X-Frame-Options"), "{page:.300}");
    // What another site asks before a request that is not plain: only reading is allowed it.
    let asks = "OPTIONS /watch HTTP/1.1\r\nOrigin: https://other.example\r\nAccess-Control-Request-Method: POST\r\n\r\n";
    let allowed = request_to(&many, asks.as_bytes()).1;
    assert!(allowed.contains("Access-Control-Allow-Methods: GET\r\n"), "{allowed}");
}

#[test]
fn nothing_is_changed_at_another_sites_asking() {
    let many = app(Matches::Many(Sessions::new(false, 1, 20.0)));
    let body = "connect=192.0.2.1%3A27020+tv";
    let post = |from: &str| {
        let send = format!(
            "POST /watch HTTP/1.1\r\nHost: stv.test\r\n{from}Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        request_to(&many, send.as_bytes()).1
    };
    for from in ["Sec-Fetch-Site: cross-site\r\n", "Sec-Fetch-Site: same-site\r\n", "Origin: https://other.example\r\n"] {
        let refused = post(from);
        assert!(refused.starts_with("HTTP/1.1 403") && refused.contains("X-Content-Type-Options: nosniff"), "{from}: {refused}");
    }
    assert!(sessions_of(&many).list().is_empty(), "no match was started");
    // Our own pages, an old browser that only says where it is from, and what is no browser.
    for from in ["Sec-Fetch-Site: same-origin\r\n", "Origin: http://stv.test\r\n", ""] {
        assert!(post(from).starts_with("HTTP/1.1 303"), "{from}");
    }
    sessions_of(&many).stop_all();
}

#[test]
fn endless_header_is_cut_off() {
    let mut send = b"GET / HTTP/1.1\r\nX-Long: ".to_vec();
    send.resize(1 << 20, b'a');
    let response = request(&send).1;
    assert!(response.starts_with("HTTP/1.1 431"), "{response:.200}");
}

/// The modules `name` imports (`from "…"` and `import("…")`), as paths under `app/`.
fn imports(name: &str, text: &str) -> Vec<String> {
    let dir: Vec<&str> = name.split('/').rev().skip(1).collect::<Vec<_>>().into_iter().rev().collect();
    let specs = text.split("from \"").chain(text.split("import(\"")).skip(1).filter_map(|r| r.split('"').next());
    specs
        .filter(|s| s.starts_with('.') && std::path::Path::new(s).extension().is_some_and(|e| e == "js"))
        .map(|spec| {
            let mut path = dir.clone();
            for part in spec.split('/') {
                match part {
                    "." => {}
                    ".." => drop(path.pop()),
                    p => path.push(p),
                }
            }
            path.join("/")
        })
        .collect()
}

#[test]
fn serves_the_page_and_its_modules_under_the_version() {
    let a = app(Matches::One(SharedHub::default()));
    let get = |path: &str| request_to(&a, format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).1;
    let page = get("/");
    let v = app_version();
    assert!(
        page.contains(&format!("\"app/{v}/style.css\""))
            && page.contains(&format!("\"app/{v}/core/faults.js\""))
            && page.contains(&format!("\"app/{v}/main.js\"")),
        "{page}"
    );
    let mut imported = std::collections::HashSet::new();
    for (name, ctype, _) in app_files() {
        let file = get(&format!("/app/{v}/{name}"));
        assert!(file.starts_with("HTTP/1.1 200 OK") && file.contains("max-age=86400"), "{name}");
        assert!(file.contains(&format!("Content-Type: {ctype}")), "{name}");
        imported.extend(imports(name, &file));
    }
    for (name, ctype, _) in app_files() {
        // The page's entries, the upload page's, and the one /embed/v1/stv-map.js loads for
        // other sites.
        let entry = ["main.js", "core/faults.js", "stv-map.js", "upload.js"].contains(&name);
        assert!(ctype != JS || entry || imported.contains(name), "nothing imports {name}");
    }
    // And everything imported is served: a module missing from APP would 404 in the browser.
    for import in &imported {
        assert!(app_files().iter().any(|(name, ..)| name == import), "{import} is imported but not in APP");
    }
    // The stylesheet's own links are relative to where it is served.
    let files = app_files();
    let css = std::str::from_utf8(files[0].2).unwrap();
    for link in css.split("url(\"").skip(1).map(|r| r.split('"').next().unwrap()).filter(|l| !l.starts_with("data:")) {
        let mut path: Vec<&str> = vec!["app", v];
        for part in link.split('/') {
            if part == ".." {
                path.pop();
            } else {
                path.push(part);
            }
        }
        let (result, font) = exchange(&a, format!("GET /{} HTTP/1.1\r\nHost: x\r\n\r\n", path.join("/")).as_bytes());
        result.unwrap();
        assert!(font.starts_with(b"HTTP/1.1 200 OK"), "{link}");
    }
    assert!(get(&format!("/app/{v}/missing.js")).starts_with("HTTP/1.1 404"));
    assert!(get("/app/main.js").starts_with("HTTP/1.1 404"));
}

#[test]
fn serves_backpack_pictures() {
    let dir = TempDir::new("backpack");
    std::fs::write(dir.join("scattergun.webp"), b"RIFF").unwrap();
    std::fs::write(dir.join("items.json"), b"{}").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.backpack = dir.to_path_buf();
    let get = |path: &str| request_to(&a, format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).1;
    let picture = get("/backpack/scattergun.webp");
    assert!(picture.starts_with("HTTP/1.1 200 OK"), "{picture}");
    assert!(picture.contains("Content-Type: image/webp") && picture.contains("max-age=86400"));
    let items = get("/backpack/items.json");
    assert!(items.starts_with("HTTP/1.1 200 OK") && items.contains("Content-Type: application/json"), "{items}");
    for bad in
        ["/backpack/../x.webp", "/backpack/a/b.webp", "/backpack/missing.webp", "/backpack/scattergun.png", "/backpack/.webp", "/backpack/"]
    {
        assert!(get(bad).starts_with("HTTP/1.1 404"), "{bad}");
    }
}

#[test]
fn map_geometry_is_gzipped_for_browsers_that_take_it() {
    let dir = TempDir::new("mapgzip");
    let json = format!(r#"{{"floors":[{}]}}"#, r#"{"z":0,"p":[0,0,1,0,1,1],"k":"grass"},"#.repeat(200).trim_end_matches(','));
    std::fs::write(crate::maps::cache::geometry_path(&dir, "cp_test"), &json).unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.cache = dir.to_path_buf();
    let get = |accept: &str| {
        let (result, response) = exchange(&a, format!("GET /map/cp_test HTTP/1.1\r\nHost: x\r\n{accept}\r\n").as_bytes());
        result.unwrap();
        let (head, body) = head_and_body(&response);
        (head, body.to_vec())
    };
    let (head, body) = get("Accept-Encoding: br, gzip;q=0.9\r\n");
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("\r\nContent-Encoding: gzip") && head.contains("\r\nVary: Accept-Encoding"),
        "{head}"
    );
    assert!(
        head.contains("\r\nContent-Type: application/json\r\n")
            && head.contains(&format!("\r\nContent-Length: {}\r\n", body.len()))
            && body.len() < json.len() / 4,
        "{head}"
    );
    let mut unzipped = String::new();
    GzDecoder::new(&body[..]).read_to_string(&mut unzipped).unwrap();
    assert_eq!(unzipped, json);
    // Served again from the gzipped copy in the cache, which the plain JSON stays next to.
    assert!(crate::maps::cache::geometry_path(&dir, "cp_test").with_extension("json.gz").exists());
    assert_eq!(get("Accept-Encoding: gzip\r\n").1, body);
    let (head, body) = get("");
    assert!(!head.contains("Content-Encoding") && head.contains("\r\nVary: Accept-Encoding"), "{head}");
    assert_eq!(body, json.as_bytes());
}

/// A cache holding a BSP that cannot be built: it is gone once a build was tried, no network
/// needed.
fn broken_map(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new("map");
    let bsp = dir.join(format!("{name}.bsp"));
    std::fs::write(&bsp, b"not a bsp").unwrap();
    (dir, bsp)
}

#[test]
fn with_many_matches_only_a_map_being_watched_is_built() {
    let (dir, bsp) = broken_map("cp_watched");
    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let mut app = app(Matches::Many(sessions));
    app.cache = dir.to_path_buf();
    let refused = get(&app, "/map/cp_watched", "");
    assert!(refused.starts_with("HTTP/1.1 404") && refused.contains("no match is on"), "{refused}");
    assert!(bsp.exists(), "not built");
    publish(&m.hub, &server("cp_elsewhere"));
    assert!(get(&app, "/map/cp_watched", "").contains("no match is on"));
    publish(&m.hub, &server("cp_watched"));
    let tried = get(&app, "/map/cp_watched", "");
    assert!(tried.starts_with("HTTP/1.1 404") && !tried.contains("no match is on"), "{tried}");
    assert!(!bsp.exists(), "built, and dropped when it turned out broken");
}

#[test]
fn a_backpack_name_cannot_leave_its_folder() {
    let root = TempDir::new("backpackroot");
    let backpack = root.join("backpack");
    std::fs::create_dir(&backpack).unwrap();
    std::fs::write(root.join("secret.webp"), b"secret").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.backpack = backpack;
    let response = get(&a, "/backpack/../secret.webp", "");
    assert!(response.starts_with("HTTP/1.1 404") && !response.contains("secret"), "{response}");
}

/// The head of a response, which may have a binary body.
fn head_of(app: &TestApp, path: &str) -> String {
    let (result, response) = exchange(app, format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes());
    result.unwrap();
    head_and_body(&response).0
}

#[test]
fn the_fixed_files_are_served_as_what_they_are() {
    let a = app(Matches::One(SharedHub::default()));
    for (path, ctype) in [
        ("/killicons.css", CSS),
        ("/killicons.webp", WEBP),
        ("/killicons/v1/killicons.webp", WEBP),
        ("/examples/embed.html", HTML),
        ("/examples/tf2pugs.html", HTML),
        ("/examples/tf2pugs-js.html", HTML),
        ("/icons/scout.svg", SVG),
        ("/favicon.svg", SVG),
        ("/favicon.ico", "image/x-icon"),
        ("/apple-touch-icon.png", "image/png"),
        ("/fonts/RobotoCondensed-Regular.woff2", "font/woff2"),
        ("/fonts/RobotoCondensed-Bold.woff2", "font/woff2"),
    ] {
        let head = head_of(&a, path);
        assert!(head.starts_with("HTTP/1.1 200") && head.contains(&format!("Content-Type: {ctype}")), "{path}: {head}");
    }
    // The big ones a browser keeps a day.
    for path in ["/killicons.css", "/killicons.webp", "/fonts/RobotoCondensed-Bold.woff2"] {
        assert!(head_of(&a, path).contains("max-age=86400"), "{path}");
    }
    for path in ["/icons/nope.svg", "/icons/scout", "/nothing"] {
        assert!(head_of(&a, path).starts_with("HTTP/1.1 404"), "{path}");
    }
}

#[test]
fn cached_overviews_keep_and_maps_do_not() {
    let cache = TempDir::new("cachedheads");
    std::fs::create_dir(cache.join("demostf")).unwrap();
    std::fs::write(cache.join("demostf/boundaries.json"), b"{}").unwrap();
    std::fs::write(cache.join("demostf/cp_x.webp"), b"RIFF").unwrap();
    std::fs::write(crate::maps::cache::geometry_path(&cache, "cp_x"), b"{}").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.cache = cache.to_path_buf();

    let list = head_of(&a, "/overview/boundaries.json");
    assert!(list.contains("Content-Type: application/json") && list.contains("max-age=86400"), "{list}");
    let image = head_of(&a, "/overview/cp_x.webp");
    assert!(image.contains("Content-Type: image/webp") && image.contains("max-age=86400"), "{image}");
    let map = head_of(&a, "/map/cp_x");
    assert!(map.contains("Content-Type: application/json") && map.contains("Vary: Accept-Encoding") && !map.contains("max-age"), "{map}");
}

#[test]
fn pages_leave_styling_to_the_stylesheet() {
    let pages = ["index.html", "landing.html", "mumble.html", "gone.html"]
        .map(|page| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates").join(page)).unwrap());
    for page in &pages {
        assert!(!page.contains(" style=") && !page.contains("<style"), "styling in a page: put it in app/css/");
    }
}

/// A browser told of every module at once need not find them import by import.
#[test]
fn the_viewer_s_page_names_every_module_it_loads() {
    let a = app(Matches::One(SharedHub::default()));
    let page = get(&a, "/", "");
    let preloads = page.matches("<link rel=\"modulepreload\" fetchpriority=\"low\"").count();
    assert!(preloads > 40, "{preloads}");
    for module in ["main.js", "core/core.js", "panel/feed.js"] {
        let link = format!("<link rel=\"modulepreload\" fetchpriority=\"low\" href=\"app/{}/{module}\">", app_version());
        assert!(page.contains(&link), "{module}");
    }
    assert!(!page.contains("stv-map.js\">"), "the embed's entry is not the page's");
    assert!(page.find("modulepreload").unwrap() < page.find("</head>").unwrap());
}

#[test]
fn a_fault_while_signed_in_is_that_steam_ids() {
    use axum::extract::FromRequestParts;
    let (app, key, _dir) = site_app();
    let cookie = signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let events = sentry::test::with_captured_events(|| {
        let mut parts = axum::http::Request::get("/").header("cookie", &cookie).body(()).unwrap().into_parts().0;
        let _ = runtime.block_on(visitor::Who::from_request_parts(&mut parts, &app.shared()));
        sentry::capture_message("broke", sentry::Level::Error);
    });
    assert_eq!(events[0].user.as_ref().and_then(|u| u.id.as_deref()), Some("7656"));
}

#[test]
fn an_answer_has_its_trace_the_callers_if_it_sent_one() {
    crate::report::on_for_tests();
    let theirs = "0123456789abcdef0123456789abcdef";
    let answer = request(format!("GET / HTTP/1.1\r\nsentry-trace: {theirs}-0123456789abcdef-0\r\n\r\n").as_bytes()).1;
    assert!(answer.contains(&format!("X-Trace-Id: {theirs}\r\n")), "{answer}");
    // As the bot's SDK sends it, unsampled.
    let answer = request(format!("GET / HTTP/1.1\r\nsentry-trace: {theirs}-0123456789abcdef\r\n\r\n").as_bytes()).1;
    assert!(answer.contains(&format!("X-Trace-Id: {theirs}\r\n")), "{answer}");
    let answer = request(b"GET / HTTP/1.1\r\n\r\n").1;
    let id = answer.lines().find_map(|l| l.strip_prefix("X-Trace-Id: ")).unwrap_or_default();
    assert!(id.len() == 32 && id != theirs, "{answer}");
}
