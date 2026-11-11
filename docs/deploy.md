# Deploying stv.tf

`config/deploy.yml` is the Kamal config: one app per region on the serveme.tf web hosts.
`STVTF_TOKEN_KEY` comes from `~/.config/stvtf/token.key` (shared with serveme.tf).

| command | |
|---|---|
| `bin/build` | builds and pushes the one image all regions run |
| `bin/deploy [REGION...]` | deploys it, in parallel (default: all four) |
| `kamal deploy -d eu\|na\|au\|sea` | builds and deploys one region |
| `bin/relays [REGION...]` | restarts the relays at the version `bin/build` pushed (boots them the first time), only when they changed |

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

## Relays

The rooms' TF2 relays run apart from the app, as the Kamal accessory `relay`
(`stvtf-web --relay-daemon /data/relay.sock …`, same image). A deploy leaves them be: TF2 clients
stay connected and the new app picks the relays up over `/data/relay.sock` (`--relay-socket`).
A room watched only in TF2 survives a deploy too.

Restarting the relays drops their TF2 clients; `retry` gets them back (same port and password).

The app never waits on a relay: it tells it where its room is only on a change (seek, pause,
speed, camera) or a drift of 250 ms, and leaves one that does not answer alone for 2 s. A relay
nobody asks about stops after 2 minutes.

## Docker

`Dockerfile` builds the server with the directory holding both repositories as context:
`docker build -f stvtf-web/Dockerfile .` (`Dockerfile.dockerignore` keeps it to the two crates).
Listens on 8080, caches maps in `/data`, answers `/up` for health checks.
