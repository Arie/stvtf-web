# Embedding

## iframe

Put a start link (from serveme.tf's API) in an iframe. `view=overlay`, `delay=<s>` and the
[display and overlay options](viewer.md#display-options) carry over:
`/start?t=…&view=overlay&delay=5&names=1` opens `/w/<id>/overlay/5?names=1`. `side=0` leaves out
the events and chat.

The page talks to its parent with `postMessage`. Every message has `stvtf: 1` and arrives as the
map shows it, so after any delay. Players are `{id, name, steamid, steamid64, team, class}`;
`steamid` is `[U:1:…]` (`BOT` for bots, whose `steamid64` is null).

| from the map, `type` | |
|---|---|
| `ready` | the page loaded |
| `map` | `map`: loaded, also after a map change |
| `status` | `state`: the SourceTV connection (`full` is live, `map_change`, `disconnected`, …) |
| `state` | twice a second: `map`, `score` `{red, blue}`, `players` with `alive`, `hp`, `max_hp`, `weapon` (`{item, clip, image}`: item definition index, rounds loaded, picture URL; or null) |
| `event` | `event`: what the feed and chat show (`kill`, `uber`, `medic_death`, `point_captured`, `capture_blocked`, `flag`, `round_start`, `round_win`, `chat`, `voice_command`, …) as stvtf emits it, without per-shot and per-hit effects; `players`: those it names, by id |

| to the map, `type` | |
|---|---|
| `follow` | by `steamid`, `steamid64` or `id`; none or unknown stops |
| `layer` | `name` (`caps`, `heals`, `hits`, `marks`, `jumps`, `pickups`, `points`, `flow`, `loadout`, `effects`), `on` |

```js
frame.contentWindow.postMessage({ stvtf: 1, type: "follow", steamid: "[U:1:231702]" }, "https://stv.tf");
addEventListener("message", e => { if (e.origin === "https://stv.tf" && e.data?.stvtf === 1) console.log(e.data); });
```

Event fields follow stvtf's output and may change. A `kill` adds what the feed works out: `icon`
(weapon, headshot or backstab variant; null if none), `icon_box` `{url, x, y, w, h}` in the
sprite, `streak` (attacker's kills within 10 s; 2+ is a multikill) and `airshot`.

## Kill icons

Draw from the box, or with stv.tf's classes, stable within `v1`; `crit` adds TF2's red glow. Drawn
for a dark feed.

```html
<link rel="stylesheet" href="https://stv.tf/killicons/v1/killicons.css">
<span class="killicon killicon-${ev.icon} ${ev.crit ? "crit" : ""}"></span>
```

## `<stv-map>`

```html
<script type="module" src="https://stv.tf/embed/v1/stv-map.js"></script>
<stv-map src="https://stv.tf/start?t=…" side="0" style="width: 100%"></stv-map>
<script>
  const map = document.querySelector("stv-map");
  map.addEventListener("state", e => showScore(e.detail.score));
  map.addEventListener("event", e => addToFeed(e.detail.event, e.detail.players));
  map.follow({ steamid64: "76561197960497430" });
</script>
```

- `src`: a start link or match page (`https://stv.tf/w/<id>/`). `view="overlay"`, `delay`, `side`
  and the display and overlay options work as on a link.
- Fires the messages above as events named by `type`, the message as `detail`.
- `follow({steamid64 | steamid | id})` (no argument stops) and `setLayer(name, on)`.
- 16:9 unless sized; styles in a shadow root; takes the keys while focused. One per page.
- `/embed/v1/` serves the current build within 5 minutes of a release.

The server sends `Access-Control-Allow-Origin: *`; the element asks `/start?t=…&format=json`
instead of following the redirect.

## Examples

All play the demo match:

- [`examples/embed.html`](../examples/embed.html) ([live](https://arie.github.io/stvtf-web/embed.html)):
  a page built around an iframe from its messages.
- [`examples/tf2pugs.html`](../examples/tf2pugs.html) ([live](https://arie.github.io/stvtf-web/tf2pugs.html)):
  a tf2pugs.com pug page mockup; roster cards show live health and follow by `steamid64`.
- [`examples/tf2pugs-js.html`](../examples/tf2pugs-js.html) ([live](https://arie.github.io/stvtf-web/tf2pugs-js.html)):
  the same with `<stv-map>`.
