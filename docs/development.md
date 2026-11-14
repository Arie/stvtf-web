# Development

## Checks

`rust-toolchain.toml` picks the latest stable (as do CI and Docker); `rustup update` now and then.
Before pushing, run what CI runs:

```
npm ci && mise install             # once: ESLint, Prettier, tsc; shellcheck, actionlint, zizmor, hadolint
script/test                        # every check; -p in parallel
script/test --fix                  # cargo fmt, ESLint fixes, Prettier, then the rest
script/test -s checks              # a section (checks, tests, audit); -t clippy,tsc some checks, --skip others
script/test --list                 # each check and its command
```

| check | config |
|---|---|
| rustfmt | `rustfmt.toml` |
| clippy | `[lints]` in `Cargo.toml`; functions ≤ 60 lines (`clippy.toml`) |
| rustdoc | |
| ESLint | `eslint.config.mjs`: functions ≤ 60 lines, complexity ≤ 15, files ≤ 400 lines; older code over the limits in `eslint-suppressions.json` (`npx eslint app test --prune-suppressions`) |
| Prettier | `.prettierrc.json` |
| tsc | `tsconfig.json`, strict, against `types/` |
| shellcheck, actionlint, zizmor, hadolint | scripts, workflows (and their security), Dockerfile |
| tests | page tests, `cargo test`, browser test |
| cargo-deny | `deny.toml`: advisories, licences, sources (`cargo install cargo-deny`) |

A missing program skips its check locally and fails it in CI. Warnings fail CI.

CI (`.github/workflows/ci.yml`) runs `script/test` on every push to `main`, every pull request,
and weekly for advisories. It checks out `Arie/stvtf` beside this repository for the `../stvtf`
dependency, with the deploy key in `STVMAP_DEPLOY_KEY`.

Once per clone:

- `ln -s ../../.githooks/pre-commit .git/hooks/pre-commit` runs `script/test` before each commit,
  in parallel, without `cargo test`, the browser test and cargo-deny. (Not `core.hooksPath`, which
  would replace global hooks.)
- `git config blame.ignoreRevsFile .git-blame-ignore-revs` keeps formatting commits out of blame.

## Conventions

- Numbers that decide behaviour (timings, thresholds, limits) are named constants at the top of
  their module.
- Colours are named in `app/css/palette.css`. Pages carry no styling (a test checks); it is in
  `app/css/`.
- `testdata/lines.jsonl` has one line of each kind the feed and chat show: Rust checks it against
  stvtf, the page's tests read it.

## Tests

```
cargo test
npm test
npm run test:browser    # after cargo build and `npx playwright-core install --only-shell chromium`
```

- `npm test` runs the page's modules in Node (`test/js/`, `test/js/setup.js` standing in for the
  browser).
- `npm run test:browser` (`test/browser/`) watches 25 s of `match.jsonl.gz` (players renamed) in
  headless Chromium: page, overlay, iframe and `<stv-map>`, each without an error. Offline, on
  `testdata/maps/cp_gullywash_f9.bsp`.
- BSP tests use `testdata/maps/` (`cp_process_f12`, `cp_gullywash_f9`, `pl_upward_f12`,
  `pl_swiftwater_final1`, `mge_training_v8_beta4b`, `cp_sunshine`). A missing map skips its test,
  except with `CI` set. Download one with
  `curl -o testdata/maps/cp_process_f12.bsp https://fastdl.serveme.tf/maps/cp_process_f12.bsp`.
- Tests write only in a temporary directory, removed afterwards, also on failure.

## Layout

- `src/main.rs`: start-up, match housekeeping, connections; `src/cli.rs`: the command line;
  `src/log.rs`: what the server says of itself (`--logs`).
- `src/web.rs`: HTTP: page, overlay, `/events`, `/start`, connect form, `/up`.
  `web/assets.rs`: served files (`/map/NAME`, `/overview/`, `/backpack/`, icons, fonts).
  `web/token.rs`: the site's tokens.
- `src/matches/`: `hub` (a match's state, feed, per-page queues), `sessions` (many matches, saved
  across deploys), `recording` (what rooms play), `live_demos` (live `.dem` files), `demostf`
  (demos.tf demos), `replay` (saved matches).
- `src/rooms/`: `room` (playback, people, drawing, relay), `registry` (open rooms, saved),
  `people`.
- `src/relays/`: `pool` (rooms as SourceTV addresses) and `daemon` (relays in their own process).
- `src/voice/`: voice from outside a match: `feed` and `hold` (held to its tick), `mumble`
  (channels the bot is wanted in), `who` (who is who, between regions), `links` (Discord to
  Steam), `ids`. Its routes: `web/voice_*.rs`.
- `src/maps/`: `cache` (geometry and overviews in `--cache`) and `bsp` (top-down geometry, on
  `/map/NAME`): `format` (lumps, LZMA), `entities` (bounds, packs, capture areas, spawns),
  `material` (floor kind from texture), `displacement`, `track` (payload), `reach` (flood fill),
  `props` (tops of static props); `drape` (strokes laid on the floors, for TF2).
- `app/`: the page, embedded in the binary (canvas, ES modules, no bundler). `build.rs` lists its
  modules and joins the stylesheet's parts.
  - `main.js` draws each frame; `stv-map.js` is `<stv-map>`; `style.css` lists `css/` in order;
    `pages/` has `index.html` and `landing.html`.
  - `core/`: `core.js` (state, options, tick clock), `stream.js` (event stream), `env.js` (own
    page or `<stv-map>`), `input.js` (keys, mouse, follow), `layers.js` (overkill settings).
  - `map/`: `view.js` (load, frame, rotate, background), `geometry.js`, `camera.js` (auto zoom);
    `players.js`, `status.js`, `projectiles.js`, `effects.js`, `buildings.js`, `spawns.js`,
    `overkill.js` draw; `events.js` turns events into effects. `map/objectives/`: `objectives.js`
    → `points.js`, `payload.js`, `modes.js`, `flags.js`.
  - `panel/`: `hud.js` (cards, scoreboard), `feed.js` (events, chat), `speakers.js` and `voice*.js`
    (the speakers list, comms).
  - `room/`: `room.js` (bar, people, drawing), `roomview.js` (shared view), `playbar.js`,
    `timeline.js`, `telestrator.js` (strokes).
  - `embed/embed.js`: talks to an embedding page.
- `types/`: stream lines and page state as `.d.ts`, for tsc only.
- `script/test`: every check. `bin/build`, `bin/deploy`, `bin/relays`: the image and the relays,
  see [deploy](deploy.md).

## Map backgrounds

- **demos.tf overview**: the images and bounds the [demos.tf viewer](https://demos.tf/viewer)
  uses (`maps.demos.tf`), looked up its way; about 50 competitive maps. Fetched once into
  `--cache` (default `cache/`) and served from there. Misaligned bounds are corrected in
  `app/map/view.js`.
- **map geometry**: any map, from its BSP on `fastdl.serveme.tf/maps/`. Upward faces coloured by
  material (grass, dirt, concrete, metal, wood, tile, water, lava, snow), displacements at real
  height, wall outlines, the payload track. Only the reachable area: a flood fill from spawns,
  capture areas, pickups and the track, without buried and shallow water; maps without points or
  pickups (MGE, jump) draw every floor. Framed by points, pickups and nearby spawns (or the
  floors) to keep the 3D skybox out.

  The JSON: floors with kind `k` (and `d` for terrain, `g` the slope along x and y), `props` (the
  tops of solid static props, from `props.txt.gz`), `track` segments, `playable` (a mask of
  16-unit cells), `packs` (health and ammo by size), `cap_areas` (trigger bounds and point).
  Cached as `<map>.v9.json` and `<map>.v9.json.gz`.

## Backpack

`backpack/` holds item pictures (`<image_name>.webp`, 64 px high) and `items.json` (item
definition index → picture), served at `/backpack/<name>`.
