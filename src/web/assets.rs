//! The files served: page modules, fonts, icons, backpacks, maps, overviews.

use super::{CACHE_FOR, JS, JSON, WEBP};
use crate::maps::cache::{map_geometry, map_geometry_gzip, overview};
use crate::{App, lock};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

/// The page's modules and stylesheets (top-level ones served with their `@import`s inlined).
#[derive(RustEmbed)]
#[folder = "app"]
#[include = "*.js"]
#[include = "*.css"]
#[include = "mumble.json"]
struct AppFiles;

#[derive(RustEmbed)]
#[folder = "fonts"]
#[include = "*.woff2"]
struct Fonts;

/// Class icons, and the site's own under `brand/`.
#[derive(RustEmbed)]
#[folder = "icons"]
#[include = "*.svg"]
#[include = "brand/favicon.*"]
#[include = "brand/apple-touch-icon.png"]
struct Icons;

#[derive(RustEmbed)]
#[folder = "killicons"]
struct Killicons;

#[derive(RustEmbed)]
#[folder = "examples"]
#[include = "*.html"]
struct Examples;

/// How long other sites keep `/embed/v1/stv-map.js`, which names the build: a deploy reaches them
/// this soon.
const LOADER_CACHE_FOR: std::time::Duration = std::time::Duration::from_mins(5);

/// What other sites' kill icons have more: the class names stay as they are within a version.
const CRITS: &str = "\n.killicon.crit { filter: drop-shadow(0 0 3px #ff4b3a); }\n";

/// Whether file `name` has extension `extension`.
fn is(name: &str, extension: &str) -> bool {
    std::path::Path::new(name).extension().is_some_and(|e| e == extension)
}

fn data<E: RustEmbed>(name: &str) -> Option<Vec<u8>> {
    E::get(name).map(|file| file.data.into_owned())
}

/// The page's files as served, by name under `app/`; `style.css` first.
pub(super) fn app_files() -> &'static [(String, Vec<u8>)] {
    static FILES: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();
    FILES.get_or_init(|| {
        let mut names: Vec<String> = AppFiles::iter().map(|n| n.to_string()).collect();
        // No part of a stylesheet (`css/`) by itself, nor what is no file of the page.
        names.retain(|n| is(n, "js") || (is(n, "css") && !n.contains('/')));
        names.sort_by_key(|n| (n != "style.css", !is(n, "css"), n.clone()));
        let served = |name: String| {
            let body = data::<AppFiles>(&name)?;
            let body = if is(&name, "css") { inline_imports(&String::from_utf8_lossy(&body)).into_bytes() } else { body };
            Some((name, body))
        };
        names.into_iter().filter_map(served).collect()
    })
}

/// A stylesheet with each `@import "x";` line replaced by the file it names.
fn inline_imports(sheet: &str) -> String {
    let part = |line: &str| {
        let name = line.strip_prefix("@import \"")?.strip_suffix("\";")?;
        Some(String::from_utf8_lossy(&data::<AppFiles>(name)?).into_owned())
    };
    sheet.lines().map(|line| part(line).unwrap_or_else(|| format!("{line}\n"))).collect()
}

/// Static file `path`, cached for a day; the page's are under `/app/<version>/`.
pub(super) fn site_file(path: &str) -> Response {
    let name = path.trim_start_matches('/');
    let (body, kept) = match name.split_once('/') {
        Some(("app", versioned)) => {
            let file = versioned.split_once('/').map_or("", |(_, file)| file);
            (app_files().iter().find(|(name, _)| name == file).map(|(_, body)| body.clone()), true)
        }
        Some(("fonts", font)) => (data::<Fonts>(font), true),
        Some(("icons", icon)) if !icon.contains('/') => (data::<Icons>(icon), false),
        Some(("examples", example)) => (data::<Examples>(example), false),
        Some(("killicons", "v1/killicons.css")) => {
            (data::<Killicons>("killicons.css").map(|css| [&css[..], CRITS.as_bytes()].concat()), true)
        }
        Some(("killicons", "v1/killicons.webp")) => (data::<Killicons>("killicons.webp"), true),
        None if name == "mumble.json" => (data::<AppFiles>(name), false),
        None if name.starts_with("killicons.") => (data::<Killicons>(name), true),
        None if name.starts_with("favicon.") || name == "apple-touch-icon.png" => (data::<Icons>(&format!("brand/{name}")), true),
        _ => (None, false),
    };
    let Some(body) = body else { return StatusCode::NOT_FOUND.into_response() };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let ctype = if mime.type_() == "text" { format!("{mime}; charset=utf-8") } else { mime.to_string() };
    let cache = kept.then(|| [(header::CACHE_CONTROL, format!("max-age={}", CACHE_FOR.as_secs()))]);
    ([(header::CONTENT_TYPE, ctype)], cache, body).into_response()
}

/// The voice bot's Mumble certificates, as JSON.
pub(super) fn mumble_bots() -> Option<Vec<u8>> {
    data::<AppFiles>("mumble.json")
}

/// `/embed/v1/stv-map.js`: a fixed address for other sites, loading this build's viewer.
pub(super) fn embed_loader() -> Response {
    let loader = format!("export * from \"../../app/{}/stv-map.js\";\n", app_version());
    let headers = [(header::CONTENT_TYPE, JS.to_string()), (header::CACHE_CONTROL, format!("max-age={}", LOADER_CACHE_FOR.as_secs()))];
    (headers, loader).into_response()
}

/// `/overview/<file>`: an overview or the maps' boundaries, cached, fetched on a miss.
pub(super) fn cached_overview(app: &App, file: &str) -> Response {
    let _one_at_a_time = lock(&app.overviews);
    match overview(&app.cache, file) {
        Ok(body) => {
            let ctype = if file == "boundaries.json" { JSON } else { WEBP };
            ([(header::CONTENT_TYPE, ctype.to_string()), (header::CACHE_CONTROL, format!("max-age={}", CACHE_FOR.as_secs()))], body)
                .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// `/map/<name>`: a map's geometry, cached, gzipped if taken; built on a miss for a live map.
pub(super) fn map(app: &App, name: &str, gzip: bool) -> Response {
    // Only a watched map is built, so only those lock.
    let fetch = app.on_map(name);
    let map_lock = fetch.then(|| app.map_lock(name));
    let _one_at_a_time = map_lock.as_deref().map(lock);
    let built = if gzip { map_geometry_gzip(&app.cache, name, fetch) } else { map_geometry(&app.cache, name, fetch) };
    match built {
        Ok(body) => {
            let headers = [(header::CONTENT_TYPE, JSON), (header::VARY, "Accept-Encoding")];
            (headers, gzip.then_some([(header::CONTENT_ENCODING, "gzip")]), body).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// Changes whenever a page file does, so browsers holding `/app/` files for a day fetch new ones
/// after a deploy; the modules' relative imports carry it along.
/// The viewer's modules, without the embed's and upload page's entries.
pub(super) fn page_modules() -> &'static [String] {
    static MODULES: OnceLock<Vec<String>> = OnceLock::new();
    MODULES.get_or_init(|| {
        let modules = app_files().iter().map(|(name, _)| name.clone());
        modules.filter(|name| is(name, "js") && !["stv-map.js", "upload.js"].contains(&name.as_str())).collect()
    })
}

/// The page files' version: their hash (FNV-1a), so a deploy busts the day's cache.
pub(crate) fn app_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| format!("{:016x}", fnv1a(app_files().iter().flat_map(|(_, body)| body.iter().copied()))))
}

/// FNV-1a, 64 bits.
fn fnv1a(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

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
