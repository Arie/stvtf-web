# stvtf-web

The server behind [stv.tf](https://stv.tf): a live top-down view of a TF2 match in the
browser. It links [stvtf](../stvtf) as a library: one SourceTV connection per match, fanned
out to any number of browsers over Server-Sent Events.

```
cargo build --release
./target/release/stvtf-web [--public] [--max-matches 20] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
./target/release/stvtf-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
./target/release/stvtf-web --replay FILE.jsonl [--listen 127.0.0.1:8080]
```

Without a server, open `http://127.0.0.1:8080/` and paste a SourceTV connect string (or paste
it into the terminal). Each match gets its own link, `/w/<id>/` with its overlay at
`/w/<id>/overlay`; the same server asked for again joins the match already being watched.
A match nobody has watched for 3 minutes is disconnected. `--max-matches` caps how many are
watched at once, and `--public`, for a server that strangers can use, refuses private and
internal addresses.

Behind a site such as serveme.tf, which signs users in and decides what they may watch, run
it with `--public --base-path /live` and `STVTF_TOKEN_KEY` (64 hex digits) shared with the
site. Matches then start only from the site's `/start?t=TOKEN` links: the token is AES-256-GCM
(`base64url(12-byte nonce | ciphertext | tag)` of `{"connect", "owner", "exp"}`), so the
SourceTV password in it is never readable in a URL, and a user (`owner`) watches one match at
a time; a null owner is unlimited.

`--sessions DIR` keeps a file per running match there (encrypted with `STVTF_TOKEN_KEY` when
set), and on start picks the matches up again under the same links, with their feed so far:
a restart or deploy is then a pause of a second or two for the people watching. SIGTERM, like
Ctrl-C, disconnects every match cleanly first.

With any number of matches, it fetches the demos.tf overviews and builds the geometry of the
maps in `maps.txt` (the most played on serveme.tf) when it starts, so their first viewer does not wait for the BSP download; other
maps are built when a match on them is first viewed (and no others, so nobody can fill the cache
with every map on fastdl). Only the geometry stays in the cache, not the BSP.

`--demo MATCH.jsonl[.gz]` adds a fixed match at `/w/demo/`: a saved `stvtf` output played on
a loop, to show the viewer off without a live game. It needs no token, is never disconnected,
does not count towards `--max-matches` and holds still while nobody is watching.

stv.tf runs this way: `config/deploy.yml` is its Kamal config (`kamal deploy -d eu|na|au|sea`),
one app per region on the serveme.tf web hosts, with `STVTF_TOKEN_KEY` read from
`~/.config/stvtf/token.key` (shared with serveme.tf). It pushes to and pulls from ghcr.io with
the classic token in `~/.config/stvtf/ghcr.token`, which has only the `write:packages` scope,
because Kamal leaves the registry login on every host.

`Dockerfile` builds that server, with the directory holding both repositories as context:
`docker build -f stvtf-web/Dockerfile .` (`Dockerfile.dockerignore` keeps the context to the
two crates). It listens on 8080, keeps the map cache in `/data` and answers `/up` for health checks.

The connect string can be as serveme.tf shows it (`connect HOST:PORT; password "tv"`),
`HOST:PORT tv`, `HOST; password tv` or just `HOST`. Without a port it uses 27020; given a
game port, it tries game port + 5.

Given a server, it watches just that match at `http://127.0.0.1:8080/`. `--replay` plays a
saved `stvtf connect` output at its recorded speed, which is handy when working on the page.

The map zooms to where the players are fighting. Dragging or scrolling takes over
the camera; the "Auto zoom" button hands it back.

Stop it with Ctrl-C: the first one disconnects every match from SourceTV cleanly (otherwise the
server holds the slot for 300 s), a second one exits at once.

## Broadcast overlay

`/overlay` (or `/w/<id>/overlay`) is a minimap for streams, like the live radar in CS
broadcasts: add it to OBS as a Browser source of any size (for example 480x480 or
560x315). The page is transparent apart from a rounded panel; it has no controls and
frames itself around the spawns and points of the map, upright like the main view, and
then glides in on the area where the living players are.

It shows the map, players as team dots with class icon, view direction and health ring,
an X where a player died until they respawn, übers, heal beams, sentries, projectiles, the
intelligence (or bball ball) at home, dropped or on its carrier,
shots and explosions, control points (squares filling left to right while captured) and
the active spawn rooms.

Query options, e.g. `/overlay?names=1&score=1&panel=0.6`:

| option | |
|---|---|
| `names=1` | player names under the dots |
| `score=1` | a small scoreboard (scores, round timer, points) at the top |
| `panel=0..1` | opacity of the panel background (default 0.82; 0 for none) |
| `zoom=map` | keep the whole map in view instead of zooming to where the players are |
| `rotate=auto\|<degrees>` | map rotation (default 0, upright like the main view; `auto` turns a quarter when that fills the panel clearly better) |
| `ui=<factor>` | scale all markers (default 1; marker size already follows the panel size) |
| `dot=<px>` | fixed player dot radius instead |

## What it shows

- A spectator HUD over the map like TF2's: BLU down the left, RED down the right.
  Each player card has the class icon, name, ping, big health number and a health
  bar; low health pulses, overheal is light blue, dead players are greyed out, and a
  player under a charge gets a striped card with its type (ÜBER, KRITZ, QUICK-FIX,
  VACC · BULLET/BLAST/FIRE). Every medic has an ÜberCharge meter with their medigun
  (Vaccinator in four pips plus the selected resistance), READY when full and a
  draining striped bar while the charge is active.
- On the map: team-coloured dots with class icon, view direction, health ring, name;
  translucent when cloaked; a heal beam from each medic to their target (thicker
  while charging); a distinct ring per charge type (über glow, kritz crackle,
  quick-fix pulse, vaccinator shield with its resistance); small badges for burning, Jarate,
  Mad Milk, bleeding, marked for death and stun.
- Projectiles; buildings labelled like the engineer HUD (S1-S3, MS, D1-D3, T1-T3, a ring on
  teleporter exits, faint while under construction, SAPPED; the team colour drains from the top as health drops); a sentry's turret turns with
  its aim, a dashed line marks its target while attacking and each shot draws a tracer; control
  points as squares like the TF2 HUD (owner colour, filled left to right while being
  captured); the payload cart.
- Events at their tick: hitscan shot tracers, explosions, damage numbers, kills
  (marker plus a line from the attacker), spawns, ubers, captures and round banners.
- A TF2-style scoreboard at the top: team scores, the round timer (SETUP, ROUND OVER,
  OVERTIME; one per team on King of the Hill) and the control points with owner, lock and
  capture progress.
- Click a player on the map or their card in the HUD to keep them centred; drag the
  map, press Esc or click them again to stop.
- Keys: `h` hides the player HUD and scoreboard, `s` just the scoreboard, `e` the events and
  chat, and `g` switches between the demos.tf overview and the map geometry. The map takes
  the space they leave. `o` switches to the broadcast overlay and back. `?`, or the ? button,
  lists them. `k` (or the gear button) opens the overkill layer settings, `x` switches all of them
  on or off, `j` the jump trails and `l` the loadouts; choices are kept per browser. All layers are on by default (the overlay shows capture areas only). Layers that
  need detail lines make the page ask the stream for `?detail=1`.
- Spawn rooms from the map file: each team's active one in team colour, the rest as dashed
  outlines. A spawn point tied to a control point is enabled while its team owns that point
  (TF2's rule), and a team spawns in its most forward enabled room (farthest from its main
  spawn); once a team has spawned since the last change of ownership, that room is used.
  On a cp_sunshine recording this rule predicted the room of all 51 spawns outside round
  resets, where the spawns themselves correct it.
- With a demos.tf overview available, a corner switch flips between it and the map geometry.
- Side panel: an event feed stamped with the time into the round (kills with DOMINATING, REVENGE,
  AIRSHOT and FEIGN tags, übers deployed and ready, dropped übers, captures and blocks,
  destroyed buildings, banners, round start/win with the score, stalemate,
  overtime, match end, joins and leaves, map changes) and chat.
- The server keeps the feed and chat of the current map (up to 1000 events), so a page
  opened or reloaded mid-match starts with them.
- `/events?detail=1` (and `/w/<id>/events?detail=1`) also streams the high-volume lines
  (`heal`, `blast_jump`, `pickup`, `impact`, `mark`, `voice_command`, `server_text`). They are
  never kept in that history, so a page only sees them from when it connects.
- A map change clears the HUD, scoreboard, followed player and everything else from the old map.

Snapshots are interpolated and events are fired when playback reaches their tick,
about 150 ms behind the newest data, so everything lines up with the movement.

## Map backgrounds

- **demos.tf overview**: the overview images and world bounds the
  [demos.tf viewer](https://demos.tf/viewer) uses (`maps.demos.tf`), looked up the
  same way it does. About 50 competitive maps. The server fetches them once into `--cache`
  (default `cache/`) and serves them itself, so viewers do not each load them from demos.tf.
- **map geometry**: for any map, drawn from the map's BSP from fastdl
  (`fastdl.serveme.tf/maps/`): upward faces shaded by height, wall outlines, and the
  playable area framed by control points, pickups and nearby spawns (or by the floors, on
  maps without either) so the 3D skybox stays out. It also carries `packs` (health and ammo
  kits by size) and `cap_areas` (each capture trigger's bounds and its point). The extracted
  geometry is cached in `--cache`.

## Backpack pictures

`backpack/` holds the item pictures (`<image_name>.webp`, 64 px high) and `items.json`, which maps
an item definition index to its picture. The server serves them at `/backpack/<name>`; `--backpack DIR`
sets the directory (default `backpack`, `/usr/share/stvtf/backpack` in the Docker image).

## Layout

- `src/main.rs`: the SourceTV watcher thread, a small HTTP server (page, `/events`, `/map/NAME`).
- `src/bsp.rs`: BSP reading (including LZMA-compressed lumps), top-down geometry, and the
  spawn rooms and spawn points (with their control point) served with it on `/map/NAME`.
- `index.html` and `app/`: the page, its stylesheet and ES modules (canvas, no build step), embedded in the binary.

## Tests

```
cargo test --release
```

The BSP tests run against `cache/cp_process_f12.bsp` and are skipped when it has
not been downloaded yet (run the viewer on a cp_process match once, or
`curl -o cache/cp_process_f12.bsp https://fastdl.serveme.tf/maps/cp_process_f12.bsp`).

## Development

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first use.
Before pushing, run what CI runs:

```
cargo fmt                          # format (settings in rustfmt.toml)
cargo clippy --all-targets         # lints (configured under [lints] in Cargo.toml)
cargo test
cargo deny check                   # advisories, licences, sources (deny.toml; cargo install cargo-deny)
```

`ln -s ../../.githooks/pre-commit .git/hooks/pre-commit` once per clone runs fmt and clippy before
each commit (not `core.hooksPath`, which would replace any global git hooks).

CI (`.github/workflows/ci.yml`) runs these on every push to `main` and every pull request,
and weekly for new advisories. Warnings fail CI. It checks out `Arie/stvtf` next to this
repository for the `../stvtf` path dependency, using the deploy key in the
`STVMAP_DEPLOY_KEY` secret.

## Credits

From [demos.tf](https://demos.tf) by Robin Appelman: [tf-demo-parser](https://codeberg.org/demostf/parser)
(MIT or Apache-2.0), which stvtf decodes SourceTV data with, and the map overview images and
their boundaries from `maps.demos.tf`, with demos.tf's way of matching a map name to them.

Class icons (`icons/`) are by lyreix of the TF2Stadium team, licensed
CC-BY-NC-SA-4.0 (`icons/LICENCE`), taken unmodified from the
[demos.tf frontend](https://codeberg.org/demostf/frontend), except the heavy's ammo
belt, which is our own.

Backpack pictures (`backpack/`) come from TF2's game files, made by tf2-icons.

The match at `/w/demo/` is the koth_product decider of the ETF2L Season 51 Premiership upper
bracket final, MANDEM vs D M ([etf2l.org/matches/91982](https://etf2l.org/matches/91982/)),
from ETF2L's STV demo.

The HUD font is Roboto Condensed by Christian Robertson, Apache License 2.0 (`fonts/LICENSE`).

Kill icons (`killicons/`) are TF2's own kill feed icons plus those from
[Consistent & Missing Kill Icons 2025](https://gamebanana.com/mods/591386) by lennyfaic,
generated by [tf2-icons](https://github.com/Arie/tf2-icons) (copy `dist/killicons.webp`
and `dist/killicons.css` here after regenerating).
