//! Lists the page's files in `app/` for the server to embed (`APP` in `src/web/assets.rs`), so a
//! module moved or added there needs no list kept by hand. Each stylesheet at the top
//! (`style.css`, the page's, first) is served as one file: its `@import`s of `css/` are put in
//! their place. `pages/` holds the HTML, which the server takes itself.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=app");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("app");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let mut sheets: Vec<String> = fs::read_dir(&root)
        .expect("app/")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| Path::new(n).extension().is_some_and(|e| e == "css"))
        .collect();
    sheets.sort_by_key(|n| (n != "style.css", n.clone()));
    let mut entries = String::new();
    for name in &sheets {
        let sheet = out.join(name);
        fs::write(&sheet, inline_imports(&root.join(name))).expect("writing a stylesheet");
        writeln!(entries, "    ({name:?}, CSS, include_bytes!({:?})),", sheet.display().to_string()).expect("a string");
    }

    let mut modules = Vec::new();
    walk(&root, &mut modules);
    modules.sort();
    for path in &modules {
        let name = path.strip_prefix(&root).expect("under app").to_string_lossy().replace('\\', "/");
        writeln!(entries, "    ({name:?}, JS, include_bytes!({:?})),", path.display().to_string()).expect("a string");
    }
    let list = format!(
        "/// The page's stylesheet and modules, served at `/app/<version>/`: `main.js` imports the rest.\n\
         pub(super) const APP: [(&str, &str, &[u8]); {}] = [\n{entries}];\n",
        sheets.len() + modules.len()
    );
    fs::write(out.join("app_files.rs"), list).expect("writing app_files.rs");
}

/// The JavaScript modules under `dir`.
fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("app/").flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, found);
        } else if path.extension().is_some_and(|e| e == "js") {
            found.push(path);
        }
    }
}

/// The stylesheet at `path` with each `@import "x";` line replaced by the file it names.
fn inline_imports(path: &Path) -> String {
    let text = fs::read_to_string(path).expect("style.css");
    let dir = path.parent().expect("a folder");
    text.lines()
        .map(|line| match line.strip_prefix("@import \"").and_then(|l| l.strip_suffix("\";")) {
            Some(part) => fs::read_to_string(dir.join(part)).unwrap_or_else(|e| panic!("{part}: {e}")),
            None => format!("{line}\n"),
        })
        .collect()
}
