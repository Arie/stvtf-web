# Deploying stv.tf

`config/deploy.yml` is the Kamal config: one app per region on the serveme.tf web hosts.
`STVTF_TOKEN_KEY` comes from `~/.config/stvtf/token.key` (shared with serveme.tf).

| command | |
|---|---|
| `bin/build` | builds and pushes the one image all regions run |
| `bin/deploy [REGION...]` | deploys it, in parallel (default: all four) |
| `kamal deploy -d eu\|na\|au\|sea` | builds and deploys one region |
| `bin/relays [--now] [REGION...]` | restarts the relays at the version `bin/build` pushed (boots them the first time); for a change to them |

The registry is ghcr.io, with the classic token in `~/.config/stvtf/ghcr.token`, scope
`write:packages` only, because Kamal leaves the login on every host.

## Voice from Discord

Every region takes voice from the voice bot (its own repository, `stvtf-voice-bot`, with a Kamal
config of its own) with `STVTF_VOICE_KEY`, from `~/.config/stvtf/voice.key`
(`openssl rand -hex 32`); without the file a region takes none. Europe is where people link
Steam (`/voice/link`), for all regions: its Discord application's client id and secret come from
`~/.config/stvtf/discord-client.id` and `discord-client.secret`, and the application's OAuth2
redirect is `https://stv.tf/voice/linked`. The links are kept in `/data/sessions/voice-links.json`.
`STVTF_VOICE_BOT` (in `config/deploy.yml`) is where the bot is added to a Discord server, shown
with the invite.

A Mumble server must be out on the internet: one on the machine itself or its network is
refused, by stv.tf and by the bot. At a LAN, where the Mumble server is on the network there,
set `STVTF_MUMBLE_ANYWHERE=1` for both.

Who is who in a channel (a Discord user or a Mumble certificate, and the SteamID someone put
them on) is kept on Europe too, in `/data/sessions/voice-who.json`: the other regions name it
with `STVTF_VOICE_HOME` (in their `config/deploy.<region>.yml`), tell it what is said on their
pages and ask it every half minute what changed, with the bot's key.

A Discord user in a channel whom nothing is known of is looked up at the serveme.tf sites
(`STVTF_LINK_SITES` in `config/deploy.yml`; all four, by every region): each is asked in turn
who that one user is (`<site>/discord_links/<id>`, with a token for that user sealed with
`STVTF_TOKEN_KEY`), and what the first that knows them says reaches the other regions through
Europe. They are asked again after a day, or after ten minutes when no site knew them. A caster's
word on who is who stands over a link; a match starter's does in their own match, and is a
suggestion elsewhere.

Voice held for a match's delay goes over a deploy: the old container saves what it holds
(`/data/sessions/held-<match>-<process>`) every 5 s while it changes and as it stops, and the new
one plays it at its tick. After a crash what came in since the last save is gone, as is what was
due in the 7 s after it; the rest plays, late by no more than 30 s. A version that does not know
these files leaves them be.

## Logs

What the app and the relays say of themselves is on the containers' own logs (`kamal app logs`,
`kamal accessory logs relay`), which go with the container at the next deploy, and with
`--logs /data/logs` (every region has it) in `/data/logs/web.log` and `relay.log` too, each line
with its time: set aside at 8 MB as `web.1.log` up to `.5`, so some 48 MB a process at most.
They say when a match, a room or a relay starts and ends, who joined a room (by Steam id when
signed in), and of each relay (by its port) the TF2 clients that came and went (by name, their
address without its end), why they left, and every jump and pause.

```
kamal app exec -d na --reuse 'tail -200 /data/logs/relay.log'
```

## Relays

The rooms' TF2 relays run apart from the app, as the Kamal accessory `relay`
(`stvtf-web --relay-daemon /data/relay.sock …`, same image). A deploy leaves them be: TF2 clients
stay connected and the new app picks the relays up over `/data/relay.sock` (`--relay-socket`).
A room watched only in TF2 survives a deploy too.

Restarting the relays (`bin/relays`) keeps their TF2 clients: told to stop, the relay process
writes each room's clients to `/data/relay.handover` (their connections, and all they were sent
since they signed on), and the next takes them in on the same ports within 25 s. They see a pause
of some seconds and a full update, no reconnect; a room that moved to another demo meanwhile
signs its clients on again, connected still. A client in its signon or a transfer at that moment
is told to `retry` (same port and password), as all are when the next process does not come in
time.

`bin/relays` leaves a region whose relays run that code already, and one whose relays are of a
version before handovers (their clients would drop): `--now` restarts those too.

A change to how rooms move between maps needs both, and the relays go first (`bin/relays`, then
`bin/deploy`): a new relay works with the old app, but a new app with an old relay shows TF2 the
level change at a map change, which can crash the client.

TF2 clients in a room of a live match play on through a deploy of stv.tf too: the next process
records the match in a demo of its own, and the relay goes on into that one with a full update,
no signon (it is the same map load of the same server). A room picked up plays on where its relay
is until it knows its own place.

A match's recording (`/data/sessions/live-<match>.gz`) has one writer. The new container reads
it, and on as it grows (the old one closes its open chunk into it every second for a minute once
the folder is the new one's, which rooms on the new one play meanwhile), and connects to the
SourceTV itself; once its stream has a tick it asks for the file at that tick
(`live-<match>.take`). A new connection's first frame can be ahead of the stream (11 s seen), so
the old one, which looks every 250 ms, writes on until its own stream is 3 s past that tick, for
20 s at most, saying so at every look (`live-<match>.beat`) and closing a chunk every second. Then
it closes the last, writes no more and says so (`live-<match>.given`), and the new one goes on
from the tick after the file's last: nothing missing, nothing twice, none of its own signon. Told
to stop before it is asked (the new one still in its signon), the old one waits to be, 12 s at
most; told meanwhile, it records on until it has given, 15 s at most: its demo gets to that tick
too.

The old one gives its files as it stops too, and writes one again, whole, when the container
that took it is stopped first (a deploy that failed) or is gone without a word: the writer
touches `live-<match>.beat` every second, and 10 s without is gone. A file whose writer says
nothing for 3 s after the asking (a crash; a version before this gives at once, at whatever
tick) is taken as it is, as is any file 23 s after: what the old one had not written and the
time nobody was connected, or its stream was short of the tick asked at, are a hole. A stretch
an older version wrote twice is read once.

A room at live stays there: more than 3 s behind its recording's end (the file's tail come at
once) it goes to the end, as it does when the file is taken, and it is picked up at live.

A match running at a deploy goes on: the recording and its demos are picked up, and demos named
without a load (`live-<match>-<tick>.dem`) are given loads by their order in time, a lower first
tick than the file before starting the next. A version without maps does not read live
recordings and markers saved with them: a rollback to one starts them afresh, without markers.

The app never waits on a relay: it tells it where its room is only on a change (seek, pause,
speed, camera) or a drift of 250 ms, and leaves one that does not answer alone for 2 s. A relay
nobody asks about stops after 2 minutes.

## Docker

`Dockerfile` builds the server with the directory holding both repositories as context:
`docker build -f stvtf-web/Dockerfile .` (`Dockerfile.dockerignore` keeps it to the two crates).
Listens on 8080, caches maps in `/data`, answers `/up` for health checks.
