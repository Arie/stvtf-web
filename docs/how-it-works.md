# How stv.tf works

SourceTV and demos as TF2 has them, what that rules out, and what stvtf and stv.tf add.
Every number here was measured; [how](#how-this-was-checked) is at the end.

## SourceTV

srcds's built-in spectator server. Viewers connect to `tv_port` and only receive.

| cvar | default | league configs | |
|---|---|---|---|
| `tv_port` | 27020 | | stvtf tries game port + 5 when given a game port |
| `tv_delay` | 30 | 90 | seconds behind the game; measured 30.4 s for a `say` at 30 |
| `tv_snapshotrate` | 16 | 66 | frames per second: every 4 ticks, or every tick |
| `tv_transmitall` | 0 | 1 | all entities, or only what the director sees; below |
| `tv_relayvoice` | 1 | 0 | players' voice to SourceTV |

League configs: serveme.tf's copies (38 of them set `tv_delay 90` and `tv_relayvoice 0`). All 16 STV
demos checked ran `tv_transmitall 1`; 13 have a frame every tick (all 7 from serveme.tf), 2 every 4
ticks, 1 every 2.

### `tv_transmitall`

With 0, SourceTV sends only the entities the director's camera can see, plus those always sent to
a spectator: TF2 sends every player and building to the spectator team
(`CTFPlayer::ShouldForceTransmitsForTeam`). With 4 bots standing in both spawns on `ctf_turbine`,
the first full update had 128 entities at 0 and 234 at 1: the same players, flags and objectives,
but fewer doors, props, beams and `CBaseAnimating`. Projectiles were not tested.

A TF2 client watching locks its camera to the director while it is 0 (`C_HLTVCamera::IsPVSLocked`
is `!tv_transmitall`), as there is nothing to see elsewhere. stv.tf wants 1, which leagues set.
Whether voice depends on it was not tested: it needs a player talking.

## A frame

One datagram per frame: `net_Tick`, `svc_PacketEntities`, and whatever happened since: game events
(`svc_GameEvent`), temp entities (shots, explosions), user messages, sounds, string table updates.

`svc_PacketEntities` is either a **full update** (every entity) or a **delta** from an earlier tick
it names: entities entering, props changed, entities leaving. Its length field is 20 bits.

An entering entity's props are sent as changes from a **baseline**: its class's static one (the
`instancebaseline` string table), or one of two the stream sets itself (`updated_base_line`), which
the client must ack before the server uses it.

The client gets no send tables: `svc_ClassInfo` says "create on client" (in every demo and live
session checked), so the client builds its classes from its own game. Hence a client must be of
the same build, which it proves with the `SendTable` CRC in `clc_ClientInfo`. stvtf, which has no
game, takes the tables from a demo of that build.

## Live: deltas from what the client acked

The client acks a tick; the server deltas the next frame from that tick. Ack -1 and it sends a full
update of the moment. That is all the say a client has: there is no asking for an earlier moment,
so a live SourceTV only goes forward.

- stvtf acks every 100 ms: in 868 live frames at 16/s, 468 were from the frame before, 399 from two
  back.
- A lost frame breaks the chain: stvtf acks -1 until a full update comes.
- In 55 s the server set a dynamic baseline 134 times. stvtf never acks them, so every frame stays on
  the static one.
- stvtf joins as an HLTV client (`is HLTV` in `clc_ClientInfo`): `tv_status` counts it as a proxy.
  One that vanishes without a goodbye keeps its slot for 300 s (freed between 284 and 300 s),
  whatever `tv_timeout` (30) says. So stvtf always says goodbye.

## Demos: deltas from the frame before

A `.dem` is a 1072-byte header, the signon (the same messages a live client gets: `ServerInfo`,
string tables, `ClassInfo`, `SignonState` New, PreSpawn, Spawn), the data tables, the string tables,
a sync tick, then one packet per frame, then a stop.

All 16 STV demos checked have **one full update, at their first frame**; every frame after deltas
from the one before, and none uses a dynamic baseline. So:

- A moment can only be reached by decoding from the start.
- A demo can be cut at its end, never its start or middle (`examples/cut_demo.rs` in stvtf).

Demos a player records (POV) are different: deltas mostly 4 to 13 ticks back, as their client
acked, and alternating baselines.

## What TF2 cannot do

- Go back in a live SourceTV: the protocol is forward only.
- Go back in a demo reliably: TF2 reloads the map and fast-forwards from the start to the moment,
  and often fails (seen in the TF2 client, not measured here).
- Watch a match together under control: SourceTV viewers all see the same live moment and nobody
  can pause or rewind it; a demo plays for one viewer.

## stvtf: SourceTV in, JSON out

A headless SourceTV client: the netchannel, the signon, tick acks, recovery from loss. It decodes
each frame with [tf-demo-parser](https://codeberg.org/demostf/parser) and writes JSON lines:
snapshots at most 20 a second (in whole ticks, never more often than frames come) and every event.
One connection takes one slot, whoever watches. See stvtf's README, "Output".

## stv.tf: the browser

- The hub keeps a match's latest state and the current map's feed (1000 events), and fans the lines
  out to every page over Server-Sent Events.
- A match is recorded in gzipped chunks of 10 s, each starting from a state a page can join with, so
  a seek replays at most one chunk. A live match keeps its last 3 hours, across its maps, in a
  file the next process goes on from, at the tick the one before stopped writing it (see
  [deploy](deploy.md)).
- The timeline, private playbacks and rooms play from that recording. See [rooms](rooms.md).

## stv.tf: TF2 clients in a room

A room is also a SourceTV address. TF2 connects to it as to any SourceTV, and sees the room's moment:
paused, seeking, at its speed. A relay serves it, playing a `.dem`:

- **demos.tf**: the demo's own file, kept for the next to play it (see
  [site](site.md#demostf-demos)).
- **Live**: stvtf records the stream it gets as a `.dem` as it goes: the build's data tables, the
  signon of the connection, one frame per datagram with every message as the server sent it, the
  header rewritten every 100 ms so it plays while it grows. One file per connection. Its frames
  delta from what stvtf acked, not the frame before; the decoder keeps the last 128 frames for that.

The room sets the relay's moment (tick, paused, speed, the player to follow), and again whenever it
drifts more than 250 ms.

### Jumping

A thread decodes the demo as soon as it is opened, following a growing one, and keeps a
**checkpoint every 2000 ticks** (30 s): the string table changes so far, a full update of the frame
the next one builds on, the frames since, the file position. Relays on the same demo share them.

When the room's moment is more than 1 s off, the relay jumps: it decodes from the last checkpoint
before the target without sending, then gives every client:

1. A **full update written by stvtf** from the decoded entities: each entity entering with the props
   that differ from its class's static baseline.
2. After a jump back, the string table entries (`userinfo`, `instancebaseline`) that changed since,
   as they were then.
3. The last round start, win panel or game over event, so the HUD shows the round as it was.
4. The demo's own frames from there.

The client stays connected; only a demo of another map load signs it on again. The demo the next
process records of the same load (a deploy) is gone on into with a full update. Jumps measured 54
to 122 ms, from the moment set to the client having it.

### Other fixes

- **Spawn count**: in a demo the signon states' count can differ from `ServerInfo`'s
  (`ctf_turbine.dem`: 3 and 2), which a game client refuses. The relay sends `ServerInfo`'s.
- **Pause**: no frames, and `svc_SetPause`.
- **Camera**: following a player sends the director's cue (`hltv_chase`) for them, every 3 s, and
  leaves out the demo's own cues.
- **Builds**: a demo whose class tables are not the current build's is refused, and the room says
  so (an old `comp.dem`: "a TF2 build with other class tables, which a game client cannot play").

## How this was checked

On 2026-11-09, with a probe built on stvtf and tf-demo-parser:

- **Demos**: the 10 STV demos in stvtf's `tests/fixtures/` and the 6 STV and 3 POV demos in
  tf-demo-parser's `test_data/`. Counted per demo: full updates and where they are, delta sources,
  frame spacing, baselines, signon messages, `ClassInfo`'s create flag, spawn counts.
- **Live**: `serveme/tf2-cloud-server` in Docker, build 10682184, `ctf_turbine` with 12 bots and
  default cvars. Read the cvars over rcon, recorded 55 s as a `.dem` and counted it like the demos,
  timed a `say` through the stream, compared `tv_transmitall` 0 and 1 (12 roaming bots, then 4
  standing in the spawns, counting each full update's entities by class), killed a client with `kill -9`
  and watched `tv_status` every 15 s.
- **Jumps**: a relay playing `pl_upward.dem` with an stvtf client, timing each jump until a
  snapshot of the target tick arrived.
- **Source SDK 2013**: `CServerGameEnts::CheckTransmit`, `CTFPlayer::ShouldForceTransmitsForTeam`,
  `C_HLTVCamera::IsPVSLocked`. The engine's SourceTV server is not in it.
- **Configs**: serveme.tf's league configs, and the `SetConVar`s in each demo's signon.
- **Code**: the constants named here, in stvtf (`relay/play`, `decode/world.rs`, `session.rs`,
  `net/messages.rs`, `demo/`) and stv.tf (`matches/`, `rooms/room/relay.rs`).

Not checked, as it needs a TF2 client: the client behaviour the relay works around (refusing
another spawn count, following director cues).
