# Rooms

A room is a match watched together under control. Watch together on a match page (live, demos.tf
demo, `--demo`/`--demos`) opens one at `/r/<id>/` (overlay `/r/<id>/overlay`) and copies its link;
pressed again, it returns to your room on that match. The match's own page never changes.

- The opener owns it and is its first admin. Admins make others admin; the owner stays one.
- Admins pause (space), set speed (the speed button steps 1×, 2×, 4×) and seek (← → by 5 s); everyone sees the same.
- Admins move the room to another map of a live match ([Maps](#maps)).
- The marker (✒︎, `d` or `m`) draws on the map for everyone, the room's overlay too, until
  cleared (⌫ or `c`); in TF2 a fence along the floors, with an arrowhead at its end.
  Each admin picks a colour by the swatch after their name; `r` draws in red, `b` in blu, `y` in
  their own again (kept in the browser). A tool that is on shows in that colour.
- A jump arc (↷ or `a`): two clicks, where the jump starts and where it lands, draw a rocket or
  sticky jump between the floors under them, straight up then over and down, shaded dark to
  bright by its height; in TF2 too, there in the drawer's colour throughout. The tool stays on for the next arc, until Esc, its button or
  another tool. An arc stays, with the marker, until cleared.
- A circle (◯ or `t`): each click puts a small ring on the floor there, "stand here"; in TF2 a
  ring fence. It stays on for the next, and the circles stay with the marker until cleared.
- The spotlight (☀ or `f`): a click lights the floor there in your colour, "look here"; the next
  click moves it. In TF2 it is a light on the floor (one at a time is all a TF2 client shows;
  not seen with dynamic lights off). It stays with the marker until cleared.
- A note (T, or `n`): a short text (40 characters) where you click with T on, or where the pointer is with `n`. Over a player (their card in
  the HUD, their name in the events or chat, their dot) it goes on them and follows them (hidden while they are dead); else
  it goes on the floor there. Type it; Enter or clicking away places it, Esc lets it go. In TF2 it is the game's
  text bubble, shown again after every full update. Notes, 10 at most, stay with the marker until cleared.
- Voice from outside the match (a team's Discord, casters: see the viewer's Voice) plays in TF2
  too: the relay sends it as the player's own voice, or for someone who is no player in a slot nobody
  is in (TF2 shows them as unconnected; each their own, so two talk at once), and only on a full
  server as SourceTV's, one at a time. TF2 plays injected voice at a level of its own that
  wanders, and a voice of no player may fall silent there after a while (its talking mark still
  shows): the page plays every voice as sent, and is the one to listen to.
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

## Maps

A live match is a list of maps, one per map load, each its own part of the recording. A room is on
one of them. A map ends where the server says it is changing; the next begins there, nameless
until the server names it. The same map loaded again is another map.

- When the server changes map, the room stays: playback runs to the map's last moment and waits
  there, that moment still showing. Seeks stay inside the map.
- A room that was following live when its map ended, and that nobody has touched since (a seek, a
  pause, a speed, a drawing, a note, a clear, a view chosen, or the camera moved by hand), moves to
  the next map by itself once that map has play to show, and lands at live. Any other room waits.
- Admins move the room to any map kept: the latest at live, another at its start: for a map after
  another, its first snapshot, paused or not, in TF2 too (nothing of the map before). A seek to
  before it lands there. A map no longer kept, or not named yet, is refused: `Map gone`.
- The marker is kept per map: it does not show on another map, and is there again when the room
  comes back.
- The bar says `Map over`, then `Next map ready`. Admins get a `Next map` button and, with more
  than one map, a maps menu. Live on a map that is over goes to the next map.
- The match's own page follows live as before; a viewer's own rewind there stays on its map, and
  Live takes it back.

## Recording

The match is kept in memory in 10-second chunks, each starting from a joinable state, so a seek
replays at most one. Rooms on a match share it. A live match records from first watched: a room
starts at live, can go back to the start, and waits at live rather than passing it (it counts as a
viewer). A part is kept per map load, within the last 3 hours. A room counts towards
`--max-matches` and ends a minute after its last viewer.

## Across restarts

With `--sessions`, each room is saved once a second (sealed; a demos.tf demo's output beside it as
`demo-<number>.gz`), and the next process picks it up when its pages reconnect: position, pause,
speed, people, admins, view, its map and whether it moves by itself, the marker of each map. A live
recording goes there as `live-<match>.gz`, a chunk at a time, its parts with it, so only the last
seconds before a restart are lost; it goes when the match ends. A saved room nobody returns to
within an hour is dropped (swept every 5 minutes).

## In TF2

With `--relay-ports 27100-27199 --relay-host HOST` (and `--sessions`), each room is a SourceTV
address: the sidebar shows "In TF2", `connect HOST:PORT; password "PW"` and a `steam://connect`
link. A TF2 client there sees the room's moment as a spectator with a free camera: paused,
seeking and at speed with it.

- A room gets a port and random password when its page (not overlay) opens, kept until it ends,
  through deploys ([relays](deploy.md#relays)) and restarts (same port while free). Someone in TF2
  keeps the room open.
- A room that moves to another map signs the TF2 clients on to that map's demo (a map load).
- A relay that cannot start (ports full, demo not there yet) is retried; one that stops is replaced.
- It plays the match's `.dem` (stvtf's README, "Playing a demo"): a demos.tf demo's as
  `demo-<number>.dem`, a live match's as `live-<match>-<load>-<tick>.dem`, one per server
  connection, `<load>` the number of the map load it is of (0 the first; another map, or ticks
  starting over, is a new load), those of earlier loads kept for a room on an earlier map. Written
  only with `--relay-ports`. A live match's go when it ends; a demo's an hour after a room last
  played it, a live one's 3 hours after last written (swept every 5 minutes).
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
| `POST /r/<id>/map` | admin: `{"map": <n>}`, the room to that map: the latest at live, another at its start; 400 `Map gone` when it is no longer kept or not named yet |
| `GET /r/<id>/marks` | the marks of the room's map |
| `GET /w/<id>/timeline` | the match page's timeline, and its `marks`, of the newest map that has play |
| `POST /r/<id>/control` | admin: `{"paused", "speed", "seek"}` (any; `seek` in ms) |
| `POST /r/<id>/arc` | admin: `{"from": [x, y], "to": [x, y]}`, a jump arc between the floors under them, `from` the launch; 400 with why when there is no floor there, they are too close or the map is not built |
| `POST /r/<id>/circle` | admin: `{"at": [x, y]}`, a circle on the floor under it; 400 with why when there is no floor there or the map is not built |
| `POST /r/<id>/light` | admin: `{"at": [x, y]}`, the spotlight on the floor under it, from where it was; 400 with why when there is no floor there or the map is not built |
| `POST /r/<id>/note` | admin: `{"text", "at": [x, y]}` (on the floor there) or `{"text", "player": <id>}` (following them); 400 with why when it is empty or too long, there is no floor there, the map is not built or the room has its 10 notes |
| `POST /r/<id>/draw` | admin: `{"stroke": <id>, "points": [[x, y, ms], …]}`, up to 256, ms into the stroke; later points continue it; pages draw at that pace, 40 ms behind |

## Stream lines

The room's event stream adds:

| line | |
|---|---|
| `relay` | `connect`, `port`, `password` while it has one; `problem`: why it cannot play |
| `marker` | the room's map's permanent marker: `strokes` (`stroke`, `by`, `points`, `ink`; a jump arc's `arc`, its points `[x, y, z]` from launch to landing; a circle's `circle`, its centre `[x, y, z]`; the spotlight's `light`, its spot `[x, y, z]`; a note's `note`, its `text` and `player`, its spot the point), `max` (8000 points); on join, empty too |
| `arc` | a jump arc drawn: `stroke`, `by`, `ink`, `points` (`[x, y, z]`, launch to landing) |
| `circle` | a circle put down: `stroke`, `by`, `ink`, `at` (`[x, y, z]`, its centre on the floor) |
| `light` | the spotlight put down, in place of where it was: `stroke`, `by`, `ink`, `at` (`[x, y, z]`, on the floor) |
| `note` | a note put down: `stroke`, `by`, `ink`, `text`, and `at` (`[x, y, z]`, on the floor) or `player` |
| `clear` | |
| `view` | as posted, with `by` |
| `people` | `id`, `name`, `admin`, `owner`, `signed`, and `ink`, `steamid` when set |
| `draw` | as posted, with `by` |
| `playback` | `paused`, `speed`, `at`, `length` (ms), `start` (first moment kept; of a map after another, from its first snapshot), `origin` (map start), `rounds`, all the room's map's; `live` (the newest map of a match still going); `map` (its number); `over` (the map has ended); `maps` (`n`, `map`, `ready` of each map kept that has a name); on change and every second while playing |
| `seek` | before the state at the new position |
