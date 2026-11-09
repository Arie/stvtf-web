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

With a key set, matches start only from start links; the SourceTV password is never readable in
a URL.

`--home URL` sends visitors of `/` to the site (each serveme.tf region has its own `/stv` page);
without it `/` answers 403.

## `/matches`

`{"matches": [...]}`, per match: id (link `/w/<id>/`), reservation, demo, owner, server address
(never the password), map, hostname, round state and clocks, score, players with team and class,
viewers now, at most and in all, seconds watched, seconds since the latest snapshot.

## demos.tf demos

Fetched, decoded once (one at a time), kept in memory only as gzipped stvtf output. Plays once at
game speed, holding still while unwatched; everyone who opens it watches together. Ends like a
live match, 3 minutes after its last viewer; not picked up after a restart. Counts towards
`--max-matches`. Page: `/w/d<id>/`. Once ended, that link sends the visitor to `--home` with the
demo filled in.

## Sign-in

From a room ("Sign in with serveme"): `/signin?return=PATH` → the site's
`/stv/sign_in?return=PATH` → `/signed_in?t=TOKEN&return=PATH`. The user is kept in a sealed
`stvtf_user` cookie for 30 days; `/signout` drops it. Signed in, a person is their Steam id in
rooms (admin rights follow them to any device) with the site's name, linking to their Steam
profile. Streamers may make themselves admin in any room.
