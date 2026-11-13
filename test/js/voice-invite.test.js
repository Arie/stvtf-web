import assert from "node:assert/strict";
import { test } from "node:test";
import { joinCommand } from "../../app/panel/voiceinvite.js";
import { serverOf } from "../../app/panel/mumbleform.js";

test("the command to paste in Discord: the page's link with the invite, the delay only when it is not 90", () => {
  const page = new URL("https://stv.tf/w/abc/");
  assert.equal(
    joinCommand(page, "0123456789abcdef", 90),
    "/stv join link:https://stv.tf/w/abc/?voice=0123456789abcdef",
  );
  assert.equal(
    joinCommand(new URL("https://stv.tf/r/def/"), "0123456789abcdef", 0),
    "/stv join link:https://stv.tf/r/def/?voice=0123456789abcdef delay:0",
  );
  assert.equal(joinCommand(page, "0123456789abcdef", 30).endsWith(" delay:30"), true);
});

test("a Mumble server as people write it: a name, with a port or without, a link or not", () => {
  assert.deepEqual(serverOf("mumble.example.com"), { host: "mumble.example.com", port: 64738 });
  assert.deepEqual(serverOf(" mumble.example.com:1234 "), { host: "mumble.example.com", port: 1234 });
  assert.deepEqual(serverOf("mumble://mumble.example.com:1234/Team%20A"), { host: "mumble.example.com", port: 1234 });
  assert.deepEqual(serverOf("203.0.113.9"), { host: "203.0.113.9", port: 64738 });
  for (const bad of ["", "not a server", "host:99999", "host:0", "http://x/y z"])
    assert.equal(serverOf(bad), null, bad);
});
