# Behind a site

Behind a site like serveme.tf, which signs users in and decides what they may watch:
`--public --base-path /live`, and `STVTF_TOKEN_KEY` (64 hex digits) shared with the site.

## Tokens

All tokens are AES-256-GCM, `base64url(12-byte nonce | ciphertext | tag)` of a JSON object with
`exp`. Start tokens may add `"reservation"`, linking the match to the site's reservation. A start token
does not list, a list token starts nothing.

| token | |
|---|---|
| `{"connect", "owner", "exp"}` | `/start?t=TOKEN` starts or joins that match. `owner` (Steam id or null) is logged, limits nothing. |
| `{"demo": <id>, "owner", "exp"}` | plays that [demos.tf](https://demos.tf) demo, see below |
| `{"list": true, "exp"}` | `GET /matches` with `Authorization: Bearer TOKEN` |
| `{"signin": {"uid", "name", "streamer"}, "exp"}` | sign-in, see below |

With a key set, matches start from start links, and from the front page (`/`: a connect string,
a demos.tf link or an upload) for whoever is signed in, as theirs. The SourceTV password is never
readable in a URL.

`--home URL` is the site: where people sign in, and where an ended demo's link leads.

## `/matches`

`{"matches": [...]}`, per match: id (link `/w/<id>/`), reservation, demo, owner, server address
(never the password), map, hostname, round state and clocks, score, players with team and class,
viewers now, at most and in all, seconds watched, seconds since the latest snapshot.

## demos.tf demos

Fetched to disk (many at a time) and decoded (one at a time: it holds the file in memory) to
gzipped stvtf output; the page says how far that is (`downloading 40%`, `decoding`). With
`--sessions` the output is kept there as `demo-<id>.gz` (and the file as `demo-<id>.dem`, where
relays play it), so the next to open the demo waits for neither: for a month since last played,
and 5 GB of them at most (the longest unplayed go first). Plays once at
game speed, holding still while unwatched; everyone who opens it watches together. Ends like a
live match, 3 minutes after its last viewer; not picked up after a restart. Counts towards
`--max-matches`. Page: `/w/d<id>/`. Once ended, that link sends the visitor to `--home` with the
demo filled in.

## Uploaded demos

The front page takes a `.dem` (512 MB at most; or a `.zip` with one in it, or a `.gz`,
unpacked to no more than that, whatever it says of itself) from whoever is signed in (where nobody
signs in, from anyone) and plays it as a match of its own at `/w/demo/<id>/` (a player's own at
`/w/pov/<id>/`), where a room is opened on it like on any other. A `SourceTV` demo, or a player's own: that one has only what
that player's game was sent, so players out of their sight stand where they were last seen, and
it is not relayed to TF2. The same file sent again, by anyone, is the one kept.

Kept as a demos.tf demo is (30 days since last played, within the same 5 GB), and one of one
person: another takes its place.
While it is kept its link plays it again.

It is sent in parts of 32 MB at most (Cloudflare takes no more in a request): `POST /upload`
`{"size"}` gives `{"id", "part"}`; `PUT /upload/<id>?at=<bytes so far>` adds a part (`409` with
the bytes there are, when that is not where it goes on: a part whose answer was lost is not
added twice); `POST /upload/<id>` `{"name"}` gives `{"page"}`. One not finished within the hour
is dropped.

## Sign-in

From a room ("Sign in with serveme"): `/signin?return=PATH` → the site's
`/stv/sign_in?return=PATH` → `/signed_in?t=TOKEN&return=PATH`. The user is kept in a sealed
`stvtf_user` cookie for 30 days; `/signout` drops it. Signed in, a person is their Steam id in
rooms (admin rights follow them to any device) with the site's name, linking to their Steam
profile. Streamers may make themselves admin in any room.

## Mumble bots

`/mumble` lists the certificates the voice bot signs in to Mumble servers by (name and SHA-1), for
a server's admins to know it by or register; `/mumble.json` is the same for a script (`name`,
`sha1`, `sha256`). Both are `app/pages/mumble.*`, written from the certificates when those change.
