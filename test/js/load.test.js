import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { test } from "node:test";

// An import of something another module no longer exports only fails when the browser links it.
test("every module of the page links and runs", async () => {
  const modules = readdirSync(new URL("../../app/", import.meta.url)).filter(f => f.endsWith(".js"));
  assert.ok(modules.length > 10, modules.join());
  for (const m of modules) await import(`../../app/${m}`);
});
