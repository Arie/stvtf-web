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

Voice from outside the match (a team's Discord, through the voice bot): `POST /w/<id>/voice` with
`Authorization: Bearer <STVTF_VOICE_KEY>` and `{"invite": "<the match's>", "steamid": "[U:1:N]", "frames": ["<base64 Opus>",
…], "delay": <seconds>}` (up to 32 frames of 20 ms; `delay` is how far the stream is behind the
game where it was said, 90 unless said: each post its own, so a team's channel and others can
feed one match). It is held until the stream reaches the tick it was said at (the tick now plus
the delay in ticks), an utterance starting at its tick and running on by the clock, and then
comes out as `voice` lines with `injected`, for a live match only. What is said while the game is
paused is kept in the recording (`pause`), and played to no one. With `"delay": 0` it is
commentary on the stream itself (casters watching it): straight out, paused or not. 401 without
the key, 403 without the match's invite, 404 with no key set or no live match, 409 before the
stream has a tick, 503 as the server stops (the next one takes it).

A post that waited says how long, `"late": <seconds>`: that much comes off its delay, so it is
held to the tick of when it was said.

What is held goes on to the next process: each saves what it holds of a match in the store
(`held-<match>-<process>`, every 5 s while it changes and as it stops), and the one the store
belongs to takes the others' and plays it at its tick. What fell due with no process there goes
out at once, unless over 30 s late.

The invite is what lets a Discord channel into a match: the key only says it is our bot, which
anyone can have in their server. `GET /w/<id>/voice-invite` (a room's too) gives `{"invite",
"bot"}` to whoever started the match and to the site's streamers (no content to others; where nobody
signs in, to anyone): `bot` is where the bot is added to a server (`STVTF_VOICE_BOT`).
Everyone in a channel brought in so is heard.

Instead of `steamid` a post may say who talks where they talk: `"discord": "<who>"` (a Discord
user id; for Mumble `mumble:<server>:<who they are there>`), with a `"name"` and the `"channel"`
they are in. Until put on a player such a voice is nobody's, heard as of no player, on the team
most of its channel's players are on; on the page it is listed by its name, under that team or
under Others (two minutes after its last word, no longer).

Comms are set up in the speakers list, by whoever started the match and by the site's streamers
(where nobody signs in, by anyone): "Comms" swaps the list for a wider view. With no channel
brought in yet it asks Discord or Mumble. Discord: the `/stv join` command with the page's link
and the match's invite to copy, for their own Discord or to pass on to a team, with the delay
when it is not 90 s. Mumble: a server (its name, with a port if not 64738), its password and a
channel, and the bot joins at once. Once the bot is in a channel, those it is in show: how each
goes, a way to add another (the other team's), for Mumble a way to move the bot (the same form,
filled in; no password typed is the one given before) or take it out, and who is who: everyone in the channel, heard yet or not, each
with the player they are. Someone is put on a player by whoever has a say (a player of the match,
a Steam ID as people write them, or nobody), by what was said of them in an earlier match, or
by their name, when just one player's looks like it: that last is a guess, shown as one until
confirmed.

| | |
|---|---|
| `GET <page>voice-invite` | `{"invite", "bot"}`: the match's invite, and where the bot is added to a Discord server (`STVTF_VOICE_BOT`) |
| `GET <page>voice-comms` | `{"discord": {"status"} or null, "mumble": [{"id", "host", "port", "channel", "delay", "status"}]}`: each Mumble channel the bot is wanted in (at most four) |
| `POST <page>voice-mumble` | `{"host", "port", "password", "channel", "delay"}` asks the bot into one more channel (each team's, on one server or several; no password: the one given for that server); with the `id` of one it is in, moves that; `{"leave": true}` takes it out of all, with an `id` out of that one |
| `GET <page>voices` | everyone in the channel or heard: `id` (who they are there), `name`, `steamid` (whom they are put on), `linked` (their own Discord link), `heard`, `confirmed` |
| `POST <page>voice-match` | `{"discord", "player": <id>}`, `{"discord", "steamid": "<any form>"}`, or `{"discord"}` alone for nobody |

`<page>` is a match's (`/w/<id>/`) or a room's on it. Those with no say get no content from the
`GET`s (every page asks) and 403 from the `POST`s. The bot's own, with its key: `POST
<page>voice-status` with the invite, `{"kind": "discord" or "mumble", "status", "people": [{"id",
"name"}]}` or `{"kind", "left": true}`; and `GET /voice/mumble?since=<version>`, kept open, which
answers the moment a Mumble channel is asked for (`{"jobs": [...], "version"}`). What is asked
for is kept with the match, so a restart does not send the bot away; who was said to be whom is
kept in `voice-who.json`, and holds the next time the same person is in a channel.

Linking, so that it need not be done by hand: `GET /voice/link` signs a visitor in with Discord
(`identify connections`) and keeps the Steam account Discord verified as theirs
(`voice-links.json` under `--sessions`). The bot asks `GET /voice/who?discord=<user id>` with its
key: `{"steamid": "[U:1:N]"}` or null. Needs a Discord application: `STVTF_DISCORD_CLIENT_ID`,
`STVTF_DISCORD_CLIENT_SECRET`, and `STVTF_DISCORD_REDIRECT` (this site's `/voice/linked`, as the
application has it).


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
viewer watched, so nothing is spoiled; a demo shows all. Of a live match it is the newest map
with play; a viewer's own rewind stays on its map ([maps](rooms.md#maps)).

`?t=12:34` (from map start) opens a private playback there; the link button copies one.

| route | |
|---|---|
| `GET /w/<id>/timeline` | `{"at", "length", "live", "rounds", "marks"}`; `202 {}` while a demo is being read |
| `GET /w/<id>/marks?from=N` | `{"marks", "all"}`: marks from the Nth |
| `POST /w/<id>/rewind` | `{"at", "speed", "paused"}` → `{"page": "/r/<id>/"}`; events at `events?overlay=1`, controls at `control`, as its owner |
