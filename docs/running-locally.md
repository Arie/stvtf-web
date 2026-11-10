# Running locally

stv.tf on your own machine or LAN, with no serveme.tf account or tokens: paste a SourceTV
connect string, or play your own demos, and watch together in the browser and in TF2.

Tested on a clean Debian 13 (trixie); other Linux distributions work the same with their own
package names.

## Install

```sh
sudo apt-get install -y curl git build-essential
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"

git clone https://github.com/Arie/stvtf.git
git clone https://github.com/Arie/stvtf-web.git
cd stvtf-web
cargo build --release
(cd ../stvtf && cargo build --release)   # only needed to turn .dem files into demos
```

The two repositories must sit side by side: stvtf-web builds stvtf from `../stvtf`.

## Run

With `192.168.1.10` replaced by this machine's LAN address:

```sh
./target/release/stvtf-web --listen 0.0.0.0:8080 \
  --sessions sessions --demos demos \
  --relay-ports 27100-27109 --relay-host 192.168.1.10
```

Open `http://192.168.1.10:8080/` and paste a SourceTV connect string, such as
`connect 192.168.1.20:27020; password "tv"`. Each match gets
its own link, `/w/<id>/`, to send to others.

- **Watch together** on a match page opens a room, with you as its admin: pause, jump around the
  timeline, draw on the map (`d`), and click a player
  to have everyone follow them.
- **In TF2**: a room shows a `connect …` line and an *Open in TF2* link. TF2 clients there see
  what the room shows, paused and jumping with it; with `spec_autodirector 1` their camera
  follows the player the admin clicks.

Stop it with Ctrl-C, once: it disconnects from SourceTV cleanly. With `--sessions`, matches and
rooms pick up where they were when it starts again.

## Demos

Turn a `.dem` into a demo the viewer plays, in the `demos` folder:

```sh
../stvtf/target/release/stvtf demo demos/final.dem | gzip > demos/final.jsonl.gz
```

After a restart it is at `/w/final/`. Keep `final.dem` beside it: rooms play that file to TF2.
TF2 only plays demos recorded on its current version; a room says so when it cannot.

## Network

- Open TCP 8080 (the page) and UDP 27100–27109 (the rooms in TF2) in the firewall. Each room
  takes one UDP port while it is open, so the range is how many rooms TF2 can be in at once.
- Leave out `--public`: it refuses private and internal addresses, LAN servers among them.
- `--relay-host` should be an IP address: the *Open in TF2* link needs one.
- Map backgrounds come from the internet (overviews from demos.tf, map files from serveme.tf's
  public fastdl, cached in `cache/`). Without internet, matches play without them.

`--help` lists every option; [running](running.md) has the details.
