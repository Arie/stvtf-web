# Rooms

A room is a match watched together under control. Watch together on a match page (live, demos.tf
demo, `--demo`/`--demos`) opens one at `/r/<id>/` (overlay `/r/<id>/overlay`) and copies its link;
pressed again, it returns to your room on that match. The match's own page never changes.

- The opener owns it and is its first admin. Admins make others admin; the owner stays one.
- Admins pause (space), set speed (¼× to 4×) and seek (← → by 5 s); everyone sees the same.
- The pen (✎ or `d`) draws on the map for everyone, the room's overlay too, for a few seconds.
  Each admin picks a colour by the swatch after their name; `r` draws in red, `b` in blu, `y` in
  their own again (kept in the browser). A tool that is on shows in that colour.
- A jump arc (↷ or `a`): two clicks, where the jump starts and where it lands, draw a rocket or
  sticky jump between the floors under them, straight up then over and down, shaded dark to
  bright by its height, in TF2 too. The tool stays on for the next arc, until Esc, its button or
  another tool. An arc stays, with the marker (✒︎ or `m`), until cleared (⌫ or `c`).
- People are listed by the name they give (kept in the browser) or Guest N, known by the
  `stvtf_guest` cookie (pages only see numbers), or by Steam id when [signed in](site.md#sign-in).

## View

An admin sets how everyone sees the map:

- **free**: everyone their own camera.
- **auto**: auto zoom for all.
- **admin**: everyone sees that admin's part of the map, fitted to their screen. Any admin
  dragging or scrolling moves it; the latest leads.
- An admin clicking a player has everyone follow them at that admin's zoom; stopping returns to
  the view before.

Viewers move the camera only when free; admins always. The room's overlay follows the view and
is not listed.

## Recording

The match is kept in memory in 10-second chunks, each starting from a joinable state, so a seek
replays at most one. Rooms on a match share it. A live match records from first watched: a room
starts at live, can go back to the start, and waits at live rather than passing it (it counts as a
viewer). A new map starts over; only the last 3 hours are kept. A room counts towards
`--max-matches` and ends a minute after its last viewer.

## Across restarts

With `--sessions`, each room is saved once a second (sealed; a demos.tf demo's output beside it as
`demo-<number>.gz`), and the next process picks it up when its pages reconnect: position, pause,
speed, people, admins, view. A live recording goes there as `live-<match>.gz`, a chunk at a time,
so only the last seconds before a restart are lost; it goes when the match ends. A saved room
nobody returns to within an hour is dropped (swept every 5 minutes).

## In TF2

With `--relay-ports 27100-27199 --relay-host HOST` (and `--sessions`), each room is a SourceTV
address: the sidebar shows "In TF2", `connect HOST:PORT; password "PW"` and a `steam://connect`
link. A TF2 client there sees the room's moment as a spectator with a free camera: paused,
seeking and at speed with it.

- A room gets a port and random password when its page (not overlay) opens, kept until it ends,
  through deploys ([relays](deploy.md#relays)) and restarts (same port while free). Someone in TF2
  keeps the room open.
- A relay that cannot start (ports full, demo not there yet) is retried; one that stops is replaced.
- It plays the match's `.dem` (stvtf's README, "Playing a demo"): a demos.tf demo's as
  `demo-<number>.dem`, a live match's as `live-<match>-<tick>.dem`, one per server connection (a
  new map drops the old). Written only with `--relay-ports`. A live match's go when it ends; a
  demo's an hour after a room last played it, a live one's 3 hours after last written (swept
  every 5 minutes).
- Demos of an older TF2 build (other class tables) cannot be played to a client; the sidebar
  says so, as it says any reason the relay cannot play.
- `--demo`/`--demos` rooms are relayed if the `.dem` is beside them under the same name
  (`demo.dem` for `demo.jsonl.gz`).

## Routes

| route | |
|---|---|
| `GET /w/<id>/room` | 200 when a room can open, 409 when not (demo still fetching) |
| `POST /w/<id>/room` | opens one: `{"page": "/r/<id>/"}` |
| `GET /r/<id>/me` | `{"id", "admin", "signed", "streamer", "signin"}`: caller's number, admin, signed in, streamer, sign-in offered |
| `POST /r/<id>/claim` | a signed-in streamer makes themselves admin |
| `POST /r/<id>/name` | `{"name"}` |
| `POST /r/<id>/ink` | admin: `{"ink": "#rrggbb"}` |
| `POST /r/<id>/admin` | admin: `{"person": <number>, "admin": true\|false}` |
| `POST /r/<id>/view` | admin: `{"mode": "free"\|"auto"}`, `{"mode": "follow", "player": <id>, "w", "h"}` (area round them) or `{"mode": "admin", "cx", "cy", "w", "h", "rot"}` (area they see), with `"page"` (the tab's name). `"take": true` chooses the view; without it, a camera move, counted only while the view is of its kind. That page leads from then on |
| `POST /r/<id>/control` | admin: `{"paused", "speed", "seek"}` (any; `seek` in ms) |
| `POST /r/<id>/arc` | admin: `{"from": [x, y], "to": [x, y]}`, a jump arc between the floors under them, `from` the launch; 400 with why when there is no floor there, they are too close or the map is not built |
| `POST /r/<id>/draw` | admin: `{"stroke": <id>, "points": [[x, y, ms], …]}`, up to 256, ms into the stroke; later points continue it; pages draw at that pace, 40 ms behind |

## Stream lines

The room's event stream adds:

| line | |
|---|---|
| `relay` | `connect`, `port`, `password` while it has one; `problem`: why it cannot play |
| `marker` | the permanent marker: `strokes` (`stroke`, `by`, `points`, `ink`; a jump arc's `arc`, its points `[x, y, z]` from launch to landing), `max` (8000 points); on join, empty too |
| `arc` | a jump arc drawn: `stroke`, `by`, `ink`, `points` (`[x, y, z]`, launch to landing) |
| `clear` | |
| `view` | as posted, with `by` |
| `people` | `id`, `name`, `admin`, `owner`, `signed`, and `ink`, `steamid` when set |
| `draw` | as posted, with `by` |
| `playback` | `paused`, `speed`, `at`, `length` (ms), `start` (first moment kept), `origin` (map start), `rounds`, `live` (a growing recording); on change and every second while playing |
| `seek` | before the state at the new position |
