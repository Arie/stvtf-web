//! The files served: page modules, fonts, icons, backpacks, maps, overviews.

use super::{CSS, HTML, JS, JSON, SVG, TEXT, WEBP, WOFF2, not_found, respond, respond_cached, respond_with};
use crate::maps::cache::{map_geometry, map_geometry_gzip, overview};
use crate::{App, is_name, lock};
use std::net::TcpStream;

include!(concat!(env!("OUT_DIR"), "/app_files.rs"));
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

/// How long other sites keep `/embed/v1/stv-map.js`, which names the build: a deploy reaches them
/// this soon.
const LOADER_CACHE_FOR: std::time::Duration = std::time::Duration::from_mins(5);

/// The page's files, pictures and maps.
pub(super) fn serve_file(s: &mut TcpStream, app: &App, path: &str, gzip: bool) -> std::io::Result<()> {
    match path {
        "/fonts/RobotoCondensed-Regular.woff2" => respond_cached(s, WOFF2, include_bytes!("../../fonts/RobotoCondensed-Regular.woff2")),
        "/fonts/RobotoCondensed-Bold.woff2" => respond_cached(s, WOFF2, include_bytes!("../../fonts/RobotoCondensed-Bold.woff2")),
        // The site's mark: stv over tf, between a RED and a BLU dot (the voice bot's avatar is
        // icons/brand/bot-avatar-512.png, set in Discord by hand).
        "/favicon.svg" => respond_cached(s, SVG, include_bytes!("../../icons/brand/favicon.svg")),
        "/favicon.ico" => respond_cached(s, "image/x-icon", include_bytes!("../../icons/brand/favicon.ico")),
        "/apple-touch-icon.png" => respond_cached(s, "image/png", include_bytes!("../../icons/brand/apple-touch-icon.png")),
        "/killicons.css" => respond_cached(s, CSS, include_bytes!("../../killicons/killicons.css")),
        // For other sites: the class names stay as they are within a version.
        "/killicons/v1/killicons.css" => respond_cached(s, CSS, KILLICONS_V1_CSS.as_bytes()),
        "/killicons.webp" | "/killicons/v1/killicons.webp" => respond_cached(s, WEBP, include_bytes!("../../killicons/killicons.webp")),
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
            let max_age = format!("max-age={}", LOADER_CACHE_FOR.as_secs());
            respond_with(s, "200 OK", &[("Content-Type", JS), ("Cache-Control", &max_age)], loader.as_bytes())
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

/// Changes whenever a page file does, so browsers holding `/app/` files for a day fetch new ones
/// after a deploy; the modules' relative imports carry it along.
/// A `modulepreload` link for each of the page's modules: a browser fetches them at once, where
/// it would find them import by import (a chain 15 deep).
pub(crate) fn module_preloads() -> &'static str {
    static LINKS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINKS.get_or_init(|| {
        // Not the embed's entry, which a page does not load.
        let modules = APP.iter().filter(|(name, ctype, _)| *ctype == JS && *name != "stv-map.js");
        modules
            .fold(String::new(), |links, (name, ..)| format!("{links}<link rel=\"modulepreload\" href=\"app/{}/{name}\">\n", app_version()))
    })
}

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
