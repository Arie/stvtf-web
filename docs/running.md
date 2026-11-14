# Running

```
stvtf-web [--public] [--max-matches 20] [--base-path /live] [--home URL] [--sessions DIR]
          [--demo MATCH.jsonl[.gz]] [--demos DIR] [--listen 127.0.0.1:8080] [--hz 20]
          [--cache DIR] [--backpack DIR] [--no-warm] [--logs DIR]
          [--relay-ports 27100-27199 --relay-host HOST]          (rooms in TF2, see rooms.md)
          [--relay-socket SOCKET]                  (their relays in another process, see deploy.md)
stvtf-web --relay-daemon SOCKET --relay-ports 27100-27199 --relay-host HOST   (that process)
stvtf-web --relay-clients SOCKET                           (how many TF2 clients that process has)
stvtf-web HOST:TVPORT [--password tv] [--listen …] [--hz 20] [--cache DIR] [--backpack DIR]
stvtf-web --replay FILE.jsonl [--listen …]
```

`--name value` or `--name=value`. Unknown or repeated options are errors.

## Matches

Without a server, open `/` and paste a SourceTV connect string (or paste it in the terminal).
Each match gets `/w/<id>/`, its overlay `/w/<id>/overlay`. The same server again joins the match
already watched.

Connect strings: `connect HOST:PORT; password "tv"`, `HOST:PORT tv`, `HOST; password tv` or `HOST`.
No port means 27020; a game port means game port + 5.

Given `HOST:TVPORT`, it watches only that match, at `/`. `--replay` plays a saved
`stvtf connect` output at recorded speed, for working on the page.

- A match nobody watched for 3 minutes is disconnected, unless it is being played (2+ players on
  teams, data in the last minute): then it stays, recorded, for up to 6 hours.
- `--max-matches` caps matches at once; when full, the one unwatched longest (3 minutes at least)
  makes room.
- `--public` refuses private and internal addresses.

## Limits per person

By Steam id (start link or signed in), else by address (IPv6 by /64): at most 4 matches, 2 rooms
and 3 private playbacks. One more ends their oldest unwatched for 3 minutes (any private playback),
or is refused. Streamers, admins and this machine have no limit.

## Sessions

`--sessions DIR` keeps a file per match (sealed with `STVTF_TOKEN_KEY` when set) and picks them up
on start, same links, same feed: a restart is a pause of a second or two. Rooms are saved there
too, see [rooms](rooms.md#across-restarts).

SIGTERM or Ctrl-C disconnects every match cleanly (otherwise the server holds the slot for 300 s);
a second Ctrl-C exits at once. `--replay` has nothing to disconnect and takes the second.

`--logs DIR` keeps what the server says of itself in `DIR/web.log` too (a relay process's in
`relay.log`), see [deploy](deploy.md#logs).

## Maps

On start it fetches the demos.tf overviews and builds geometry for the maps in `maps.txt` (the
most played on serveme.tf). Other maps are built when first viewed, and only then, so nobody can
fill the cache with all of fastdl. Maps build in parallel, each once; only the geometry is cached.
`--no-warm` leaves it all to first viewers. See [development](development.md#map-backgrounds).

`--backpack DIR` holds the item pictures, served at `/backpack/<name>` (default `backpack`,
`/usr/share/stvtf/backpack` in the image).

## Fixed demos

`--demo MATCH.jsonl[.gz]` adds `/w/demo/`: a saved `stvtf` output on a loop. No token, never
disconnected, outside `--max-matches`, holds still while unwatched. `--demos DIR` does the same
for every `NAME.jsonl[.gz]` there, at `/w/NAME/` (letters, digits, `_`, `-`).
