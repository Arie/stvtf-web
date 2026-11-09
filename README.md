# stvtf-web

The server behind [stv.tf](https://stv.tf): a live top-down view of a TF2 match in the
browser. It links [stvtf](../stvtf) as a library: one SourceTV connection per match, fanned
out to any number of browsers over Server-Sent Events.

To run it yourself on a LAN, with no serveme.tf, see [RUNNING-LOCALLY.md](RUNNING-LOCALLY.md).

```
cargo build --release
./target/release/stvtf-web [--public] [--max-matches 20] [--base-path /live] [--home URL] [--sessions DIR]
                           [--demo MATCH.jsonl[.gz]] [--demos DIR] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
                           [--relay-ports 27100-27199 --relay-host HOST]   (rooms in TF2, see Rooms)
./target/release/stvtf-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
./target/release/stvtf-web --replay FILE.jsonl [--listen 127.0.0.1:8080]
```

Options take their value as `--name value` or `--name=value`. An unknown or repeated option
is an error rather than quietly falling back to its default; `--help` lists them all.

Without a server, open `http://127.0.0.1:8080/` and paste a SourceTV connect string (or paste
it into the terminal). Each match gets its own link, `/w/<id>/` with its overlay at
`/w/<id>/overlay`; the same server asked for again joins the match already being watched.
A match nobody has watched for 3 minutes is disconnected, unless it is being played (two players
or more on the teams, and data in the last minute): that one stays connected, and recorded, for up
to 6 hours with nobody watching, for those who come back to it. `--max-matches` caps how many are
watched at once; when that many are, the one nobody has watched the longest (3 minutes at least)
makes room for a new one, and `--public`, for a server that strangers can use, refuses private and
internal addresses.

One person, by the Steam id in the start link or of who is signed in, else by address (an IPv6
one by its /64), has at most 4 matches started, 2 rooms and 3 private playbacks at once. One more
ends their oldest that nobody has watched for 3 minutes (any private playback of theirs), or is
refused. Streamers and admins, and this machine, have no limit.

Behind a site such as serveme.tf, which signs users in and decides what they may watch, run
it with `--public --base-path /live` and `STVTF_TOKEN_KEY` (64 hex digits) shared with the
site. Matches then start only from the site's `/start?t=TOKEN` links: the token is AES-256-GCM
(`base64url(12-byte nonce | ciphertext | tag)` of `{"connect", "owner", "exp"}`), so the
SourceTV password in it is never readable in a URL. `owner` is the user's Steam id (or null): it
goes in the log with every match started or opened, and limits nothing. An optional `"reservation"`
(the site's reservation number) links the match to that reservation in the list of matches.

`GET /matches` with `Authorization: Bearer TOKEN`, a token of `{"list": true, "exp"}` sealed the
same way, answers `{"matches": [...]}`: per match its id (the link is `/w/<id>/`), reservation,
demo, owner, server address (never the password), map, hostname, round state and clocks, score,
players with team and class, viewers now, at most and in all, seconds watched and seconds since
the latest snapshot. A start token does not list and a list token starts nothing.

A token with `"demo": <id>` in place of `connect` plays that [demos.tf](https://demos.tf) demo
instead: fetched from demos.tf, decoded once (one demo at a time) and kept in memory only as its
gzipped stvtf output, it plays through once at game speed, holding still while nobody watches.
Everyone who opens the same demo while it plays watches it together, from where it is. It ends
like a live match, 3 minutes after its last viewer left, and it is not picked up again after a
restart. Its page is `/w/d<id>/`, the same each time: once the demo has ended, that link sends
the visitor to the site (`--home`) with the demo filled in, to play it again. `--max-matches`
counts it like any other match. `--home URL` sends visitors of `/` to
the site (each serveme.tf region uses its own `/stv` page); without it `/` answers 403.

`--sessions DIR` keeps a file per running match there (encrypted with `STVTF_TOKEN_KEY` when
set), and on start picks the matches up again under the same links, with their feed so far:
a restart or deploy is then a pause of a second or two for the people watching. SIGTERM, like
Ctrl-C, disconnects every match cleanly first.

With any number of matches, it fetches the demos.tf overviews and builds the geometry of the
maps in `maps.txt` (the most played on serveme.tf) when it starts, so their first viewer does
not wait for the BSP download; other maps are built when a match on them is first viewed (and
no others, so nobody can fill the cache with every map on fastdl). Different maps are built at
the same time, each map once. Only the geometry stays in the cache, not the BSP. `--no-warm`
leaves all of it to the first viewers.

`--demo MATCH.jsonl[.gz]` adds a fixed match at `/w/demo/`: a saved `stvtf` output played on
a loop, to show the viewer off without a live game. It needs no token, is never disconnected,
does not count towards `--max-matches` and holds still while nobody is watching.
`--demos DIR` does the same for every `NAME.jsonl[.gz]` in a folder, at `/w/NAME/` (NAME of
letters, digits, `_` and `-`).

stv.tf runs this way: `config/deploy.yml` is its Kamal config, one app per region on the
serveme.tf web hosts, with `STVTF_TOKEN_KEY` read from `~/.config/stvtf/token.key` (shared
with serveme.tf). `bin/build` builds and pushes the one image all regions run, and
`bin/deploy [REGION...]` deploys it to the given regions (all four by default) in parallel;
`kamal deploy -d eu|na|au|sea` builds and deploys a single region. It pushes to and pulls from ghcr.io with
the classic token in `~/.config/stvtf/ghcr.token`, which has only the `write:packages` scope,
because Kamal leaves the registry login on every host.

The rooms' TF2 relays run apart from the app, as the Kamal accessory `relay` on each host
(`stvtf-web --relay-daemon /data/relay.sock …`, the same image), which a deploy leaves be: a TF2
client in a room stays connected through it, and the app picks its rooms' relays up again over
`/data/relay.sock` (`--relay-socket`). `bin/relays [REGION...]` restarts them at the version
`bin/build` pushed (booting them the first time), only when the relays themselves changed: their
TF2 clients drop then, and `retry` gets them back in: a room's relay starts again on its port
with its password. A room watched only in TF2 survives a deploy too: the new app picks up rooms
with a relay once the old one stops saving them. The app never waits on the relay process: it tells a relay where its room is
only when that changes course (seek, pause, speed, camera) or drifts by a quarter second, and
leaves a relay process that does not answer alone for two seconds. A relay the app no longer asks about (a room gone with a process that
crashed) stops after two minutes.

`Dockerfile` builds that server, with the directory holding both repositories as context:
`docker build -f stvtf-web/Dockerfile .` (`Dockerfile.dockerignore` keeps the context to the
two crates). It listens on 8080, keeps the map cache in `/data` and answers `/up` for health checks.

The connect string can be as serveme.tf shows it (`connect HOST:PORT; password "tv"`),
`HOST:PORT tv`, `HOST; password tv` or just `HOST`. Without a port it uses 27020; given a
game port, it tries game port + 5.

Given a server, it watches just that match at `http://127.0.0.1:8080/`. `--replay` plays a
saved `stvtf connect` output at its recorded speed, which is handy when working on the page.

The camera keeps every living player in view, as tightly as it can: it catches up quickly
when one is leaving the view, and brings in a player who has just spawned gently. Before a
round it takes in both teams at once. Dragging or scrolling takes over the camera; the "Auto
zoom" button hands it back. The map is turned so blue's spawns are on the left and red's on the
right, as on the HUD (`rotate`, below, changes that).

Stop it with Ctrl-C: the first one disconnects every match from SourceTV cleanly (otherwise the
server holds the slot for 300 s), a second one exits at once. With `--replay` there is nothing
to disconnect, and it takes the second one.

## Broadcast overlay

`/overlay` (or `/w/<id>/overlay`) is a minimap for streams, like the live radar in CS
broadcasts: add it to OBS as a Browser source of any size (for example 480x480 or
560x315). The page is transparent apart from the map itself; it has no controls and
frames itself around the spawns and points of the map, turned like the main view, and
then glides in on the area where the living players are.

It shows the map, players as team dots with class icon and view direction,
an X where a player died until they respawn, übers, heal beams, the
intelligence (or bball ball) at home, dropped or on its carrier,
control points (squares filling left to right while captured) and
the active spawn rooms. Health rings, projectiles, buildings and shots stay out of it unless
asked for with their [display options](#display-options).

Besides the [display options](#display-options), it takes these, e.g.
`/overlay?names=1&score=1&panel=0.6`:

| option | |
|---|---|
| `panel=0..1` | opacity of a dark backdrop behind the whole panel (default 0, none) |
| `zoom=map` | keep the whole map in view instead of zooming to where the players are |
| `rotate=teams\|auto\|<degrees>` | map rotation, on the main view too: `teams` (the default) turns it a quarter or half so blue's spawns are left of red's, as on the HUD (upright when a team has none); `0` keeps it upright, north up; `auto` turns a quarter when that fills the panel clearly better |
| `ui=<factor>` | scale all markers (default 1; marker size already follows the panel size) |
| `dot=<px>` | fixed player dot radius instead |

## Display options

These work on the main view and the overlay alike: `1` shows a thing, `0` hides it, and anything
else leaves the view's default. For a minimap in the corner of a stream, for example:
`/overlay?beams=0&deaths=0&caps=0`.
On the main view they beat the settings saved in the browser for that visit without changing
them; pressing a key or using the settings panel still applies and saves as usual.

| option | shows | main view | overlay |
|---|---|---|---|
| `names` | player names under the dots | on | off |
| `facing` | the pointer on each dot towards where the player is looking | on | on |
| `numbers` | the control points' numbers, and the cap rate (×2) on a point being capped | on | on |
| `score` | the scoreboard | on | off |
| `health` | the health ring around the dots (the HUD's cards keep theirs) | on | off |
| `projectiles` | rockets, grenades, stickies and the like | on | off |
| `beams` | heal beams | on | on |
| `buildings` | sentries, dispensers and teleporters, with their shots, upgrades and repairs, and the one an engineer carries | on | off |
| `shots` | shot tracers, explosions, flames and airblasts | on | off |
| `deaths` | the X where a player fell, until they respawn; `0` also drops the main view's brief kill marks | off | on |
| `caps`, `heals`, `hits`, `marks`, `jumps`, `pickups`, `points`, `flow`, `loadout`, `effects` | the overkill layers of the settings panel; those that need detail lines make the overlay ask for them too | on | `caps` only |

`bg` picks the background; `g` switches between the overview and the geometry, and a map with
only one of them shows that one:

| `bg=` | |
|---|---|
| `overview` | the demos.tf overview (the default) |
| `geometry` | the map geometry, with its textures, walls and shadows |
| `flat` | the map geometry as a plain shape, in three shades for height |
| `outline` | only a line round the map's playable area |

`bgopacity=0..1` sets how strongly the background shows, with any of them.

## Delay

A number of seconds after the last `/` holds the match back by that much, to line up with a
stream of it (Twitch runs a few seconds behind SourceTV): `/w/<id>/5`, `/w/<id>/overlay/4.5`,
or `/5` and `/overlay/5` for a single server, up to 60. The page handles everything from the
server that much later, so it also starts that much later; `o` keeps the delay.

## Timeline

A match's page (not the overlay, nor in an iframe) has a timeline along the bottom: the match so
far with its rounds, ubers, drops and fights, a live match from when it was first watched. By
default it follows what everyone sees, at live for a live match. Moving it, pausing or another
speed gives that viewer a playback of their own from there (a private room: not listed, not saved,
at most 200 at once, outside `--max-matches`); on a live match, ● Live returns to the shared one.
A live match's timeline shows rounds, results, marks and fights only as far as the viewer has
watched, so going back spoils nothing; a demo's shows all of it. `?t=12:34` (from the map's start) opens the page in such a playback at that moment, and the link button by the map's name copies
one for where the viewer is.

| route | |
|---|---|
| `GET /w/<id>/timeline` | `{"at", "length", "live", "rounds", "marks"}`: where the shared playback is, the length and rounds so far, how many marks; `202 {}` while a demo is still being read |
| `GET /w/<id>/marks?from=N` | `{"marks", "all"}`: the timeline's marks from the Nth on |
| `POST /w/<id>/rewind` | `{"at", "speed", "paused"}`: a playback of the viewer's own from `at`, as `{"page": "/r/<id>/"}`; its events at `events?overlay=1` and controls at `control`, as a room's owner |

## Rooms

A room is a match watched together under control: Watch together on a match's page (live, a
demos.tf demo, or `--demo`/`--demos`) opens one at `/r/<id>/`, with its overlay at
`/r/<id>/overlay`, and copies its link to send (as does the link button by its name); pressed again, it goes back to
the room one owns on that match. Anyone may open one; the
match's own page never changes. The
browser that opened it owns it and is its first admin. Admins pause (space), change speed
(¼× to 4×) and jump anywhere on the timeline (← and → by 5 s), and everyone in the room sees the
same. With the pen (✎ or `d`) dragging on the map draws a line instead, which everyone in the
room (and the room's overlay) sees on the same spot of the map for a few seconds. The demo's own
page, its overlay and live matches never change.

Everyone in a room is listed beside the map, by the name they give (kept in the browser) or as
Guest N. Admins make others admin, or not; the owner stays one. Each admin's pen has its own
colour, which they pick by the swatch after their name.

An admin also sets how everyone sees the map: free (each their own camera), auto zoom (theirs
too), or the admin's view, moved by hand: auto zoom and following stop, and everyone sees the
same part of the map as on that admin's screen, fitted to theirs and moving smoothly. Any admin
dragging or scrolling moves it, the latest to move it leading. An admin clicking a player has
everyone follow that player, at that admin's zoom (which they still scroll), until they stop
following, which goes back to the view before.
Viewers cannot move the camera unless it is free; admins always can. A room's overlay follows
the view too, and is not listed. People are known by the `stvtf_guest` cookie, which only the server sees: pages know
each other by number.

Behind a site with `STVTF_TOKEN_KEY` and `--home`, people sign in with it from a room ("Sign in
with serveme"): `/signin?return=PATH` sends them to the site's `/stv/sign_in?return=PATH`,
which signs them in there if need be and sends them back to `/signed_in?t=TOKEN&return=PATH`,
the token sealed like a start token, of `{"signin": {"uid", "name", "streamer"}, "exp"}`. We keep
the user in a sealed `stvtf_user` cookie for 30 days (`/signout` drops it). In a room they are then
their Steam id instead of the browser, so their admin rights go with them to any device, and
their name is the site's, marked as such and linking to their Steam profile. A streamer (or more) on the site may make themselves
admin in any room.

A room keeps the match in memory in 10-second chunks, each starting from the state a page joins
with, so a jump replays at most one chunk. Rooms on the same match share it. A live match is
recorded from when it is first watched: a room on it starts at live and can go back to the start,
the ● Live button goes back to live, and playback waits there rather than going past it (the room
counts as one of the match's viewers). A new map starts the recording over, and only its last
three hours are kept. A room counts towards `--max-matches` and ends a minute
after its last viewer left.

With `--sessions DIR` each room is saved there too, once a second (sealed like the matches; a
demos.tf demo's output beside the rooms on it, as `demo-<number>.gz`), and the process that
replaces this one picks it up when its pages reconnect: where it was, paused or not, at the
same speed, with the same people, admins and view. A live match's recording goes there too, as
`live-<match>.gz` (each 10-second chunk added as it closes), so the match picks up with it and
goes on recording: only the last few seconds before the restart are missing. The file goes when
the match ends. A saved room nobody comes back to within an
hour is dropped (the sweep looks every five minutes).

### In TF2

With `--relay-ports 27100-27199 --relay-host HOST` (and `--sessions`), each room is also a
SourceTV address: the room's sidebar shows "In TF2" with `connect HOST:PORT; password "PW"` to
paste in the console (and a `steam://connect` link). A TF2 client there sees what the room shows,
as a SourceTV spectator with its own free camera: the room's moment, paused when it pauses,
jumping when it jumps, at its speed.

A room gets a port from the range and a random password once someone opens its page (not its
overlay), and keeps it until the room ends, through deploys when the relays run apart
(`--relay-socket`, see above) and restarts (on that port while it is free); the room stays open while someone is in TF2 too. A
relay that cannot start (every port taken, the demo not there yet) is tried again, and one that
stops is replaced. It plays the match's `.dem` (see stvtf's README, "Playing a demo"): a
demos.tf demo's file, kept beside its output as `demo-<number>.dem`, or a live match's, recorded
as it is watched as `live-<match>-<tick>.dem`, one per connection to the server (a new map drops
the old map's, as the recording does). These are only written with `--relay-ports`. A live
match's go when it ends; a sweep every five minutes drops a demo's an hour after a room last
played it, and a live match's three hours after it was last written. A demo of an older TF2
build, whose class tables are not the current ones, cannot be played to a game client: the
sidebar says so, as it says any other reason the relay cannot play.
Rooms on `--demo`/`--demos` recordings are relayed if the `.dem` they were made from is beside them under
the same name (`demo.dem` for `demo.jsonl.gz`); without one they have no relay.

| route | |
|---|---|
| `GET /w/<id>/room` | 200 when a room can be opened on the match, 409 when not (a demo still being fetched) |
| `POST /w/<id>/room` | opens one: `{"page": "/r/<id>/"}` |
| `GET /r/<id>/me` | `{"id", "admin", "signed", "streamer", "signin"}`: the caller's number in the room, whether they are an admin, signed in with the site and a streamer there, and whether signing in is offered |
| `POST /r/<id>/claim` | a streamer signed in with the site makes themselves admin |
| `POST /r/<id>/name` | `{"name"}`: the caller's name in the room |
| `POST /r/<id>/ink` | an admin's `{"ink": "#rrggbb"}`: their pen colour |
| `POST /r/<id>/admin` | an admin's `{"person": <number>, "admin": true\|false}` |
| `POST /r/<id>/view` | an admin's `{"mode": "free"\|"auto"}`, `{"mode": "follow", "player": <id>, "w", "h"}` (`w`, `h`: the map area round them) or `{"mode": "admin", "cx", "cy", "w", "h", "rot"}` (the map area they see), with `"page"` (the sending tab's name). `"take": true` chooses the view; without it, it is a camera move, which counts only while the view is of its kind. Either way that page leads from then on |
| `POST /r/<id>/control` | an admin's `{"paused", "speed", "seek"}` (any of them; `seek` in milliseconds) |
| `POST /r/<id>/draw` | an admin's `{"stroke": <id>, "points": [[x, y, ms], …]}`, up to 256 map points, each with when it was drawn (milliseconds into the stroke); a stroke's next points continue it, and pages draw it at that pace, 40 ms behind |

The room's event stream adds eight lines (`playback` also says `live`, for a live match whose recording grows): `relay` (`connect`, `port` and `password` while it has one, and `problem`: why it cannot play), `marker` (the permanent marker's `strokes`, each `stroke`, `by` and `points`, and `max`, the points the map holds, 8000; on join, empty too), `clear`, `view` (as posted, with `by`), `people` (`id`, `name`, `admin`, `owner`, `signed`, and when there is one, `ink` and `steamid` each), `draw`
(as posted, with `by`, the drawer's number), `playback` (`paused`, `speed`, `at` and `length` in
milliseconds, `start`, the first moment kept, and `origin`, where the map began, and
`rounds`, when each round started) on every change and every second while it
plays, and `seek` before the state at the new position.

## Embedding

A site embeds a match by putting a start link (from serveme.tf's API) in an iframe. The link
can say where it lands: `view=overlay`, `delay=<s>` and the display and overlay options carry over,
so `/start?t=…&view=overlay&delay=5&names=1` opens `/w/<id>/overlay/5?names=1`. `side=0` leaves
out the events and chat beside the map, for a site that shows its own.

In an iframe the page talks to its parent with `postMessage`. Every message has `stvtf: 1`,
and arrives as the map shows it, so after any delay. Players are
`{id, name, steamid, steamid64, team, class}`, with `steamid` as `[U:1:…]` (`BOT` for bots, whose
`steamid64` is null).

| from the map, `type` | |
|---|---|
| `ready` | the page loaded |
| `map` | `map`: the map loaded, also after a map change |
| `status` | `state`: the SourceTV connection (`full` is live, `map_change`, `disconnected`, …) |
| `state` | twice a second: `map`, `score` `{red, blue}`, `players` with `alive`, `hp`, `max_hp` and `weapon` (in hand: `{item, clip, image}`, the item definition index, rounds loaded and a picture URL, or null) |
| `event` | `event`: what the page's own feed and chat show (`kill`, `uber`, `medic_death`, `point_captured`, `capture_blocked`, `flag`, `round_start`, `round_win`, `chat`, `voice_command`, …) as stvtf emits it, without the per-shot and per-hit effects; `players`: the players it names, by id |

| to the map, `type` | |
|---|---|
| `follow` | keep a player centred, by `steamid`, `steamid64` or `id`; none, or an unknown one, stops following |
| `layer` | `name` (`caps`, `heals`, `hits`, `marks`, `jumps`, `pickups`, `points`, `flow`, `loadout`, `effects`), `on` |

```js
frame.contentWindow.postMessage({ stvtf: 1, type: "follow", steamid: "[U:1:231702]" }, "https://stv.tf");
addEventListener("message", e => { if (e.origin === "https://stv.tf" && e.data?.stvtf === 1) console.log(e.data); });
```

The event fields follow stvtf's output and may still change. A `kill` also has what the page's
own feed works out: `icon`, the kill icon to show (the weapon's, or its headshot or backstab
variant; null when there is none), `icon_box` `{url, x, y, w, h}`, where it is in the sprite,
`streak`, the attacker's kills within 10 s (2 and up for a multikill), and `airshot`.

Draw the icon from the box, or with stv.tf's kill icon classes, which keep their names within
`v1`; `crit` adds TF2's red glow:

```html
<link rel="stylesheet" href="https://stv.tf/killicons/v1/killicons.css">
<span class="killicon killicon-${ev.icon} ${ev.crit ? "crit" : ""}"></span>
```

The icons are drawn for a dark feed, like TF2's.

### As an element

Instead of an iframe, a site can put the map straight into its page:

```html
<script type="module" src="https://stv.tf/embed/v1/stv-map.js"></script>
<stv-map src="https://stv.tf/start?t=…" side="0" style="width: 100%"></stv-map>
<script>
  const map = document.querySelector("stv-map");
  map.addEventListener("state", e => showScore(e.detail.score));
  map.addEventListener("event", e => addToFeed(e.detail.event, e.detail.players));
  map.follow({ steamid64: "76561197960497430" });
</script>
```

`src` is a start link or a match page (`https://stv.tf/w/<id>/`). The attributes `view="overlay"`,
`delay`, `side` and the display and overlay options do what they do on a link. The element fires the
messages above as events named by their `type`, with the message as `detail`, and has
`follow({steamid64 | steamid | id})` (no argument stops following) and `setLayer(name, on)`. It is
16:9 unless sized otherwise, keeps its own styles in a shadow root, and takes the keyboard
shortcuts while it has focus. A page has one map; `/embed/v1/` loads the current build, within
5 minutes of a release.

The stv.tf server answers every request with `Access-Control-Allow-Origin: *` for this; the
element asks `/start?t=…&format=json` where a start link leads instead of following it.

### Examples

All play the demo match:

- [`examples/embed.html`](examples/embed.html) (<https://arie.github.io/stvtf-web/embed.html>): a
  page built around the map in an iframe from its messages.
- [`examples/tf2pugs.html`](examples/tf2pugs.html) (<https://arie.github.io/stvtf-web/tf2pugs.html>):
  a mockup of a tf2pugs.com pug page with the map in an iframe, whose roster cards show each
  player's live health and follow them by `steamid64`.
- [`examples/tf2pugs-js.html`](examples/tf2pugs-js.html)
  (<https://arie.github.io/stvtf-web/tf2pugs-js.html>): the same page with `<stv-map>`.

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
  capture progress. CTF counts captured intelligence, pass time goals, robot and player
  destruction their points, and Mann vs. Machine shows the wave.
- Game modes: the pass time ball (on its carrier while carried) and goals, robot destruction's
  robots (their health as on buildings) and dropped cores, and Mann vs. Machine's tank with its
  health, revive markers, money on the ground and the robots left in the wave.
- Hold Tab for a scoreboard like TF2's: each team by points, with kills, deaths, assists,
  damage, healing and ping. Dead players' cards count down to their respawn, time added to
  a round timer floats under the scoreboard, and an engineer carrying a building shows it on
  their dot.
- Click a player on the map or their card in the HUD to keep them centred; drag the
  map, press Esc or click them again to stop.
- Keys: `h` hides the player HUD, `s` the scoreboard (each on its own), `e` the events and
  chat, and `g` switches between the demos.tf overview and the map geometry. The map takes
  the space they leave. `o` switches to the broadcast overlay and back. `?`, or the ? button,
  lists them. `k` (or the gear button) opens the overkill layer settings, `x` switches all of them
  on or off, `j` the jump trails and `l` the loadouts; choices are kept per browser. All layers are on by default (the overlay shows capture areas only), and [query options](#display-options) can set them. Layers that
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
  destroyed and sapped buildings, banners, round start/win with the score, stalemate,
  overtime, match end, joins and leaves, map changes, votes called and their result, pass time
  goals, steals and blocks, robots destroyed, player destruction scores and team leaders killed,
  special delivery's rocket opening) and chat.
- The server keeps the feed and chat of the current map (up to 1000 events), so a page
  opened or reloaded mid-match starts with them.
- `/events?detail=1` (and `/w/<id>/events?detail=1`) also streams the high-volume lines
  (`heal`, `blast_jump`, `pickup`, `impact`, `mark`, `voice_command`, `server_text`, `hud_text`,
  `hint`, `pass_get`, `pass_free`, `pass_caught`, `points_scored`, `stun`, `capture_started`,
  `round_selected`, `teleported`, `building_removed`, `building_detonated`, `det_stickies`,
  `npc_damage`, `upgrade`). They are
  never kept in that history, so a page only sees them from when it connects.
- A map change clears the HUD, scoreboard, followed player and everything else from the old map.

Snapshots are interpolated and events are fired when playback reaches their tick,
about 150 ms behind the newest data, so everything lines up with the movement.

## Map backgrounds

- **demos.tf overview**: the overview images and world bounds the
  [demos.tf viewer](https://demos.tf/viewer) uses (`maps.demos.tf`), looked up the
  same way it does. About 50 competitive maps. The server fetches them once into `--cache`
  (default `cache/`) and serves them itself, so viewers do not each load them from demos.tf.
  Bounds that do not line up with their image are corrected in `app/view.js`.
- **map geometry**: for any map, drawn from the map's BSP from fastdl
  (`fastdl.serveme.tf/maps/`): upward faces coloured by what their texture is made of
  (grass, dirt, concrete, metal, wood, tile, water, lava, snow), terrain displacements at their
  real height, wall outlines and, on payload maps, the cart's track. Only the area players can
  reach is drawn: a flood fill from the spawns, capture areas, pickups and the cart track,
  with buried and shallow water left out. Maps without points or pickups (MGE, jump) skip the
  fill and draw every floor. It is framed by control points, pickups and nearby spawns (or by
  the floors, on maps without either) so the 3D skybox stays out. Each floor carries its kind
  (`k`) and, for terrain, `d`; the JSON also has the `track`'s segments, the `playable` area
  as a mask of 16-unit cells, `packs` (health and ammo kits by size) and `cap_areas` (each
  capture trigger's bounds and its point). The extracted geometry is cached in `--cache`
  (`<map>.v7.json`, and gzipped as `<map>.v7.json.gz` for browsers that take it).

## Backpack pictures

`backpack/` holds the item pictures (`<image_name>.webp`, 64 px high) and `items.json`, which maps
an item definition index to its picture. The server serves them at `/backpack/<name>`; `--backpack DIR`
sets the directory (default `backpack`, `/usr/share/stvtf/backpack` in the Docker image).

## Layout

- `src/main.rs`: starting up: what to watch, housekeeping of the matches, accepting connections;
  `src/cli.rs` the command line.
- `src/web.rs`: a small HTTP server: the page and overlay, `/events`, `/start` and the connect
  form, and `/up`; `src/web/assets.rs` the files it serves (the page's, `/map/NAME`, `/overview/`,
  `/backpack/`, icons and fonts); `src/web/token.rs` the site's start tokens.
- `src/matches/`: `hub` (a match's latest state and feed, and the queue of each page watching
  it), `sessions` (many matches in one process, and saving them across a deploy), `recording`
  (what rooms play), `live_demos` (a live match's `.dem` files), `demostf` (a demos.tf demo
  played as a match) and `replay` (saved matches).
- `src/rooms/`: `room` (one room: playback, people, drawing, its relay), `registry` (the rooms
  open, saved across a deploy) and `people` (who is in one).
- `src/relays/`: `pool` (rooms as `SourceTV` addresses, on ports of the range) and `daemon` (the
  relays in a process of their own).
- `src/maps/`: `cache` (map geometry and demos.tf's overviews, fetched once and kept in
  `--cache`) and `bsp`, top-down geometry from a BSP, served on `/map/NAME`. Its child modules:
  `format` (the file's lumps and records, including LZMA-compressed lumps), `entities` (the
  playable area's bounds, packs, capture areas, and spawn rooms and points with their control
  point), `material` (a floor's kind from its texture name), `displacement` (terrain at its real
  height), `track` (the payload cart's track) and `reach` (the flood fill of the playable area).
- `app/`: the page, embedded in the binary (canvas and ES modules, no bundler). `build.rs` lists
  its modules for the server and puts the stylesheet's parts into one file.
  - `main.js` draws each frame; `stv-map.js` is `<stv-map>` for other sites; `style.css` lists
    the parts in `css/`, in order; `pages/` holds `index.html` and `landing.html`.
  - `core/`: `core.js` (the shared state, options and tick clock), `stream.js` (the event stream
    in), `env.js` (where the page runs: its own, or `<stv-map>`), `input.js` (keys, mouse,
    follow) and `layers.js` (the overkill settings).
  - `map/`: `view.js` loads the map and frames it (rotation, the screen transform, the
    background), `geometry.js` draws its geometry, `camera.js` is the auto zoom; `players.js`,
    `status.js`, `projectiles.js`, `effects.js`, `buildings.js`, `spawns.js` and `overkill.js`
    draw what is on it; `events.js` turns events into effects. In `map/objectives/`,
    `objectives.js` hands each objective to `points.js` (control points), `payload.js` (the cart),
    `modes.js` (the other game modes') and `flags.js` (flags, the ball: what players carry).
  - `panel/`: `hud.js` (player cards and scoreboard) and `feed.js` (events and chat).
  - `room/`: `room.js` (a room's bar, people and drawing), `roomview.js` (how everyone sees the
    map), `playbar.js` and `timeline.js` (playback and its marks), `telestrator.js` (strokes).
  - `embed/embed.js` talks to a page embedding the map.
- `types/`: the stream's lines and the page's state as TypeScript declarations, for `tsc` only.
- `script/test` runs every check (see Development); `bin/build` and `bin/deploy` build and deploy the image.

## Tests

```
cargo test
npm test
npm run test:browser               # after cargo build and `npx playwright-core install --only-shell chromium`
```

`npm test` runs the page's modules in Node (`test/js/`), with `test/js/setup.js` standing in
for the browser: the feed, the HUD, the camera's helpers, the options and the embedding messages.
`npm run test:browser` (`test/browser/`) watches 25 s of a recorded match (`match.jsonl.gz`, players
renamed) in headless Chromium: the page, the overlay, an iframe and `<stv-map>`, each without an error.
It runs offline on `testdata/maps/cp_gullywash_f9.bsp`.

The BSP tests run against the maps in `testdata/maps/` (`cp_process_f12`, `cp_gullywash_f9`,
`pl_upward_f12`, `pl_swiftwater_final1`, `mge_training_v8_beta4b`, `cp_sunshine`). Each test skips
itself when its map has not been downloaded, for example with
`curl -o testdata/maps/cp_process_f12.bsp https://fastdl.serveme.tf/maps/cp_process_f12.bsp`,
except when `CI` is set, where a missing map fails it.
Tests that write files do so in a temporary directory that is removed afterwards, also when
they fail.

## Development

`rust-toolchain.toml` picks the latest stable (as do CI and the Docker build); run `rustup update` now and then.
Before pushing, run what CI runs:

```
npm ci && mise install             # once: ESLint, Prettier, tsc; shellcheck, actionlint, zizmor, hadolint (mise.toml)
script/test                        # every check, one after another; -p runs them at the same time
script/test --fix                  # cargo fmt, ESLint's fixes and Prettier, then the rest
script/test -s checks              # a section (checks, tests, audit); -t clippy,tsc picks checks, --skip leaves some out
script/test --list                 # each check and its command
```

The checks: rustfmt (rustfmt.toml), clippy (`[lints]` in Cargo.toml), rustdoc, ESLint (eslint.config.mjs;
code over its size limits when they came in is in eslint-suppressions.json, pruned with
`npx eslint app test --prune-suppressions`), Prettier (.prettierrc.json), tsc (tsconfig.json, strict, against
`types/`: the stream's lines and the page's state as .d.ts), shellcheck, actionlint, zizmor (the workflows'
security), hadolint (the Dockerfile), the page tests, `cargo test`, the browser test and cargo-deny (advisories, licences and
sources: deny.toml; `cargo install cargo-deny`). A check whose program is missing is skipped locally, and fails in CI.

`testdata/lines.jsonl` is one
line of each kind the feed and chat show: Rust checks it against stvtf, the page's tests read it.

`ln -s ../../.githooks/pre-commit .git/hooks/pre-commit` once per clone runs `script/test` before each commit, in
parallel and without `cargo test`, the browser test and cargo-deny (not `core.hooksPath`, which would replace any global git hooks). `git config blame.ignoreRevsFile .git-blame-ignore-revs`
keeps formatting-only commits out of `git blame` (GitHub skips them already).

CI (`.github/workflows/ci.yml`) runs them, through `script/test`, on every push to `main` and every pull request,
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
