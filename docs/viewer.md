# The viewer

## What it shows

- **HUD**: BLU left, RED right. Cards with class icon, name, ping, health and bar; low health
  pulses, overheal light blue, dead greyed out and counting down to respawn. A charged player's
  card is striped with its type (ÜBER, KRITZ, QUICK-FIX, VACC · BULLET/BLAST/FIRE). Medics have an
  ÜberCharge meter (Vaccinator in four pips plus resistance).
- **Players**: team dots with class icon, view direction, health ring, name; translucent when
  cloaked; heal beams (thicker while charging); a ring per charge type; badges for burning,
  Jarate, Mad Milk, bleeding, marked for death, stun. An engineer carrying a building shows it.
- **Objects**: projectiles; buildings labelled like the engineer HUD (S1-S3, MS, D1-D3, T1-T3,
  faint while building, SAPPED, team colour draining with health); sentry aim, target line and
  tracers; control points as on the TF2 HUD; the payload cart.
- **Events** at their tick: tracers, explosions, damage numbers, kills, spawns, ubers, captures,
  round banners.
- **Scoreboard**: scores, round timer (SETUP, ROUND OVER, OVERTIME; per team on KotH), control
  points with owner, lock and progress; CTF captures, pass time goals, RD/PD points, MvM wave.
  Time added floats under it. In stopwatch (tournament A/D and payload), TF2's stopwatch under
  it: the time and points the first half's attackers set, then the time left to beat it. Hold Tab
  for TF2's: kills, deaths, assists, damage, healing, ping.
- **Game modes**: pass time ball and goals, RD robots and cores, MvM tank, revive markers, money,
  robots left.
- **Spawn rooms**: each team's active one in team colour, the rest dashed. A spawn tied to a
  point is enabled while its team owns it; a team spawns in its most forward enabled room, and
  once it has spawned since the last ownership change, that room. On a cp_sunshine recording this
  predicted all 51 spawns outside round resets.
- **Side panel**: event feed stamped with round time (kills with DOMINATING, REVENGE, AIRSHOT,
  FEIGN; übers; captures and blocks; buildings; rounds, stalemate, overtime, match end; joins,
  leaves, map changes, votes; pass time, RD, PD, special delivery) and chat. The server keeps the
  current map's feed (1000 events), so a reload starts with it.

Snapshots are interpolated, events fire when playback reaches their tick, about 150 ms behind the
newest data. A map change clears everything from the old map.

`/events?detail=1` (and `/w/<id>/events?detail=1`) adds the high-volume lines (`heal`,
`blast_jump`, `pickup`, `impact`, `mark`, `voice_command`, `server_text`, `hud_text`, `hint`,
`pass_get`, `pass_free`, `pass_caught`, `points_scored`, `stun`, `capture_started`,
`round_selected`, `teleported`, `building_removed`, `building_detonated`, `det_stickies`,
`npc_damage`, `upgrade`). They are not kept in history.

`?voice=1` (rooms' `/r/<id>/events` too) adds the players' voice: `voice` lines, each the speaker
(`player`, `steamid`), its `rate` and its Opus `frames` in base64 (see stvtf's README). A page asks
for them once its viewer turns voice on. They are not kept in history.

## Voice

When the match carries the players' voice (the server relays it to SourceTV), 🔇 by the map's
Overview/Geometry switch turns it on (kept in the browser; a browser plays sound only after a
click). It opens the speakers list: voice off, mute all, ⇄ swap (hear the other team, mute this
one), RED and BLU each with a team mute and everyone on it now, heard or not, and the rest heard;
each with a mute and a volume.
A team's mute covers its players to come too, for this page; one's own mutes are kept in the
browser by SteamID, across matches. Everyone heard has a 🔊 on their HUD card (🔇 muted), faint
while silent and lit while they talk; a talking or muted player's dot has the same badge. A click
on either mutes or unmutes them (a muted team's player is then let through). Paused, sought or at
another speed than 1×, voice is silent. It needs WebCodecs, so a secure page (https, or
localhost); the overlay has none.

## Camera

Keeps every living player in view, tightly: quick when one leaves the view, gentle for a fresh
spawn, both teams before a round. Dragging or scrolling takes over; "Auto zoom" hands it back.
Click a player or their card to follow; drag, Esc or click again to stop. The map is turned so
blue spawns left (`rotate` changes that).

## Keys

| key | |
|---|---|
| `h` / `s` / `e` | player HUD / scoreboard / events and chat |
| `g` | overview or geometry (also the corner switch) |
| `o` | broadcast overlay and back |
| `k` | overkill layer settings (gear button) |
| `x` / `j` / `l` | all layers / jump trails / loadouts |
| `?` | this list |

Choices are kept per browser. All layers are on by default (the overlay shows capture areas only).
Layers that need detail lines make the page ask for `?detail=1`.

## Broadcast overlay

`/overlay` (or `/w/<id>/overlay`): a minimap for streams, as an OBS Browser source of any size
(e.g. 480x480, 560x315). Transparent apart from the map, no controls; frames the map's spawns and
points, then glides to where the living players are.

It shows players (dot, class, direction), an X where one died until respawn, übers, heal beams,
the intelligence or ball, control points and active spawn rooms. The rest is off unless asked for
with [display options](#display-options). Its own options, e.g. `/overlay?names=1&score=1&panel=0.6`:

| option | |
|---|---|
| `panel=0..1` | backdrop opacity (default 0) |
| `zoom=map` | whole map instead of following the players |
| `rotate=teams\|auto\|<degrees>` | also on the main view. `teams` (default): blue spawns left of red (upright when a team has none); `0`: north up; `auto`: a quarter turn when that fills the panel clearly better |
| `ui=<factor>` | marker scale (default 1; already follows panel size) |
| `dot=<px>` | fixed player dot radius |

## Display options

Main view and overlay: `1` shows, `0` hides, anything else keeps the default. E.g.
`/overlay?beams=0&deaths=0&caps=0`. On the main view they override saved settings for that
visit, without changing them.

| option | shows | main | overlay |
|---|---|---|---|
| `names` | names under the dots | on | off |
| `facing` | view direction pointer | on | on |
| `numbers` | point numbers, cap rate (×2) | on | on |
| `score` | scoreboard | on | off |
| `health` | health ring (cards keep theirs) | on | off |
| `projectiles` | rockets, grenades, stickies… | on | off |
| `beams` | heal beams | on | on |
| `buildings` | buildings, their shots, upgrades, repairs, carried ones | on | off |
| `shots` | tracers, explosions, flames, airblasts | on | off |
| `deaths` | X until respawn; `0` also drops the main view's kill marks | off | on |
| `caps`, `heals`, `hits`, `marks`, `jumps`, `pickups`, `points`, `flow`, `loadout`, `effects` | overkill layers | on | `caps` only |

`bg=` picks the background (a map with only one shows that one); `bgopacity=0..1` its strength.

| `bg=` | |
|---|---|
| `overview` | demos.tf overview (default) |
| `geometry` | map geometry with textures, walls, shadows |
| `flat` | geometry as a plain shape, three shades for height |
| `outline` | a line round the playable area |

## Delay

Seconds after the last `/` hold the match back, to line up with a stream: `/w/<id>/5`,
`/w/<id>/overlay/4.5`, `/5`, `/overlay/5`; up to 60. `o` keeps it.

## Timeline

On a match page (not the overlay, not in an iframe): the match so far with rounds, ubers, drops
and fights; a live match from when first watched. It follows the shared playback by default.
Seeking, pausing or changing speed gives the viewer a private playback (a room: unlisted, unsaved,
at most 200, outside `--max-matches`); ● Live returns. A live match shows marks only as far as the
viewer watched, so nothing is spoiled; a demo shows all.

`?t=12:34` (from map start) opens a private playback there; the link button copies one.

| route | |
|---|---|
| `GET /w/<id>/timeline` | `{"at", "length", "live", "rounds", "marks"}`; `202 {}` while a demo is being read |
| `GET /w/<id>/marks?from=N` | `{"marks", "all"}`: marks from the Nth |
| `POST /w/<id>/rewind` | `{"at", "speed", "paused"}` → `{"page": "/r/<id>/"}`; events at `events?overlay=1`, controls at `control`, as its owner |
