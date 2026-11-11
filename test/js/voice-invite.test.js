import assert from "node:assert/strict";
import { test } from "node:test";
import { joinCommand } from "../../app/panel/voiceinvite.js";

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
