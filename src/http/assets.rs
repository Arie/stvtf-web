//! The files served: page modules, fonts, icons, backpacks, maps, overviews.

use super::{CSS, HTML, JS, JSON, SVG, TEXT, TTF, WEBP, not_found, respond, respond_cached, respond_with};
use crate::cache::{map_geometry, map_geometry_gzip, overview};
use crate::{App, is_name, lock};
use std::net::TcpStream;

/// The page's stylesheet and modules, served at `/app/<version>/`: `main.js` imports the rest.
pub(super) const APP: [(&str, &str, &[u8]); 29] = [
    ("style.css", CSS, include_bytes!("../../app/style.css")),
    ("env.js", JS, include_bytes!("../../app/env.js")),
    ("core.js", JS, include_bytes!("../../app/core.js")),
    ("layers.js", JS, include_bytes!("../../app/layers.js")),
    ("stream.js", JS, include_bytes!("../../app/stream.js")),
    ("geometry.js", JS, include_bytes!("../../app/geometry.js")),
    ("view.js", JS, include_bytes!("../../app/view.js")),
    ("camera.js", JS, include_bytes!("../../app/camera.js")),
    ("input.js", JS, include_bytes!("../../app/input.js")),
    ("events.js", JS, include_bytes!("../../app/events.js")),
    ("feed.js", JS, include_bytes!("../../app/feed.js")),
    ("payload.js", JS, include_bytes!("../../app/payload.js")),
    ("points.js", JS, include_bytes!("../../app/points.js")),
    ("modes.js", JS, include_bytes!("../../app/modes.js")),
    ("flags.js", JS, include_bytes!("../../app/flags.js")),
    ("objectives.js", JS, include_bytes!("../../app/objectives.js")),
    ("buildings.js", JS, include_bytes!("../../app/buildings.js")),
    ("room.js", JS, include_bytes!("../../app/room.js")),
    ("telestrator.js", JS, include_bytes!("../../app/telestrator.js")),
    ("spawns.js", JS, include_bytes!("../../app/spawns.js")),
    ("overkill.js", JS, include_bytes!("../../app/overkill.js")),
    ("status.js", JS, include_bytes!("../../app/status.js")),
    ("players.js", JS, include_bytes!("../../app/players.js")),
    ("projectiles.js", JS, include_bytes!("../../app/projectiles.js")),
    ("effects.js", JS, include_bytes!("../../app/effects.js")),
    ("hud.js", JS, include_bytes!("../../app/hud.js")),
    ("embed.js", JS, include_bytes!("../../app/embed.js")),
    ("stv-map.js", JS, include_bytes!("../../app/stv-map.js")),
    ("main.js", JS, include_bytes!("../../app/main.js")),
];
const ICONS: [(&str, &[u8]); 9] = [
    ("scout", include_bytes!("../../icons/scout.svg")),
    ("soldier", include_bytes!("../../icons/soldier.svg")),
    ("pyro", include_bytes!("../../icons/pyro.svg")),
    ("demoman", include_bytes!("../../icons/demoman.svg")),
    ("heavy", include_bytes!("../../icons/heavy.svg")),
    ("engineer", include_bytes!("../../icons/engineer.svg")),
    ("medic", include_bytes!("../../icons/medic.svg")),
    ("sniper", include_bytes!("../../icons/sniper.svg")),
    ("spy", include_bytes!("../../icons/spy.svg")),
];

/// FNV-1a, 64 bits.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// The page's files, pictures and maps.
pub(super) fn serve_file(s: &mut TcpStream, app: &App, path: &str, gzip: bool) -> std::io::Result<()> {
    match path {
        "/fonts/RobotoCondensed-Regular.ttf" => respond_cached(s, TTF, include_bytes!("../../fonts/RobotoCondensed-Regular.ttf")),
        "/fonts/RobotoCondensed-Bold.ttf" => respond_cached(s, TTF, include_bytes!("../../fonts/RobotoCondensed-Bold.ttf")),
        "/killicons.css" => respond(s, "200 OK", CSS, include_bytes!("../../killicons/killicons.css")),
        "/killicons.webp" => respond(s, "200 OK", WEBP, include_bytes!("../../killicons/killicons.webp")),
        // For other sites: the class names stay as they are within a version.
        "/killicons/v1/killicons.css" => respond_cached(s, CSS, KILLICONS_V1_CSS.as_bytes()),
        "/killicons/v1/killicons.webp" => respond_cached(s, WEBP, include_bytes!("../../killicons/killicons.webp")),
        p if let Some(file) = p.strip_prefix("/icons/") => match ICONS.iter().find(|(name, _)| file.strip_suffix(".svg") == Some(name)) {
            Some((_, svg)) => respond(s, "200 OK", SVG, svg),
            None => not_found(s),
        },
        p if let Some(file) = p.strip_prefix("/app/") => {
            match APP.iter().find(|(name, ..)| file.split_once('/').is_some_and(|(_, n)| n == *name)) {
                Some((_, ctype, body)) => respond_cached(s, ctype, body),
                None => not_found(s),
            }
        }
        p if let Some(file) = p.strip_prefix("/backpack/") => serve_backpack(s, app, file),
        p if p.starts_with("/overview/") || p.starts_with("/map/") => serve_cached(s, app, p, gzip),
        // A fixed address for other sites, loading this build's viewer.
        "/embed/v1/stv-map.js" => {
            let loader = format!("export * from \"../../app/{}/stv-map.js\";\n", app_version());
            respond_with(s, "200 OK", &[("Content-Type", JS), ("Cache-Control", "max-age=300")], loader.as_bytes())
        }
        "/examples/embed.html" => respond(s, "200 OK", HTML, include_bytes!("../../examples/embed.html")),
        "/examples/tf2pugs.html" => respond(s, "200 OK", HTML, include_bytes!("../../examples/tf2pugs.html")),
        "/examples/tf2pugs-js.html" => respond(s, "200 OK", HTML, include_bytes!("../../examples/tf2pugs-js.html")),
        _ => not_found(s),
    }
}

/// A map's geometry (gzipped when the browser takes it), or a demos.tf overview, from the cache
/// (fetched and made on a miss).
fn serve_cached(s: &mut TcpStream, app: &App, p: &str, gzip: bool) -> std::io::Result<()> {
    let result = if let Some(file) = p.strip_prefix("/overview/") {
        let _one_at_a_time = lock(&app.overviews);
        overview(&app.cache, file)
    } else if let Some(name) = p.strip_prefix("/map/") {
        // Only a map being watched is built, so only those are locked: any other is read from the
        // cache, which is written whole or not at all.
        let fetch = app.on_map(name);
        let map_lock = fetch.then(|| app.map_lock(name));
        let _one_at_a_time = map_lock.as_deref().map(lock);
        if gzip { map_geometry_gzip(&app.cache, name, fetch) } else { map_geometry(&app.cache, name, fetch) }
    } else {
        return not_found(s);
    };
    match result {
        // A map's JSON is 1.5 to 3.5 MB, and about a sixth of that gzipped.
        Ok(body) if p.starts_with("/map/") => {
            let mut headers = vec![("Content-Type", JSON), ("Vary", "Accept-Encoding")];
            if gzip {
                headers.push(("Content-Encoding", "gzip"));
            }
            respond_with(s, "200 OK", &headers, &body)
        }
        Ok(body) if p == "/overview/boundaries.json" => respond_cached(s, JSON, &body),
        Ok(body) => respond_cached(s, WEBP, &body),
        Err(e) => respond(s, "404 Not Found", TEXT, e.to_string().as_bytes()),
    }
}

/// A backpack picture or `items.json`; the name is checked so it cannot leave the directory.
fn serve_backpack(s: &mut TcpStream, app: &App, file: &str) -> std::io::Result<()> {
    let found = match file.rsplit_once('.') {
        Some((stem, "webp")) if is_name(stem, b"_") => Some(WEBP),
        Some(("items", "json")) => Some(JSON),
        _ => None,
    };
    match found.and_then(|ctype| Some((ctype, std::fs::read(app.backpack.join(file)).ok()?))) {
        Some((ctype, body)) => respond_cached(s, ctype, &body),
        None => not_found(s),
    }
}

/// Changes whenever a page file does, so browsers holding `/app/` files for a day fetch new ones after a deploy;
/// the modules' relative imports carry it along.
pub(crate) fn app_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| format!("{:016x}", fnv1a(APP.iter().flat_map(|(.., body)| body.iter().copied()))))
}

fn fnv1a(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(FNV_OFFSET_BASIS, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

const KILLICONS_V1_CSS: &str =
    concat!(include_str!("../../killicons/killicons.css"), "\n.killicon.crit { filter: drop-shadow(0 0 3px #ff4b3a); }\n");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_a_hash_of_the_page_files() {
        // FNV-1a's published test vectors.
        assert_eq!(fnv1a(b"".iter().copied()), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a".iter().copied()), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar".iter().copied()), 0x8594_4171_f739_67e8);
        assert_eq!(app_version().len(), 16);
        assert!(app_version().bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
