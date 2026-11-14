import assert from "node:assert/strict";
import { test } from "node:test";
import { parts, upload } from "../../app/upload.js";

test("a file goes in parts no bigger than the server takes", () => {
  assert.deepEqual(parts(10, 4), [
    [0, 4],
    [4, 8],
    [8, 10],
  ]);
  assert.deepEqual(parts(4, 4), [[0, 4]]);
});

test("a part whose answer was lost goes on from where the server has it", async t => {
  const real = globalThis.fetch;
  t.after(() => (globalThis.fetch = real));
  /** @type {string[]} */
  const asked = [];
  let had = 0;
  globalThis.fetch = /** @type {any} */ (
    async (/** @type {URL | string} */ url, /** @type {RequestInit} */ init) => {
      const u = new URL(String(url));
      asked.push(`${init.method} ${u.pathname}${u.search}`);
      const answer = (/** @type {number} */ status, /** @type {unknown} */ body) => ({
        ok: status < 300,
        status,
        json: async () => body,
        text: async () => String(body),
      });
      if (init.method === "PUT") {
        const at = Number(u.searchParams.get("at"));
        if (at !== had) return answer(409, had);
        had += /** @type {Blob} */ (init.body).size;
        // The first part is kept, but its answer never comes.
        if (at === 0) throw new Error("lost");
        return answer(200, had);
      }
      return u.pathname === "/upload" ? answer(200, { id: "abc", part: 4 }) : answer(200, { page: "/w/d1/" });
    }
  );
  const file = /** @type {File} */ (/** @type {unknown} */ (new Blob([new Uint8Array(10)])));
  /** @type {number[]} */
  const said = [];
  const real_timeout = globalThis.setTimeout;
  globalThis.setTimeout = /** @type {any} */ ((/** @type {() => void} */ f) => real_timeout(f, 0));
  t.after(() => (globalThis.setTimeout = real_timeout));
  assert.equal(
    await upload(Object.assign(file, { name: "m.dem" }), new URL("http://stv.test/"), d => said.push(d)),
    "/w/d1/",
  );
  assert.deepEqual(asked, [
    "POST /upload",
    "PUT /upload/abc?at=0",
    "PUT /upload/abc?at=0",
    "PUT /upload/abc?at=4",
    "PUT /upload/abc?at=8",
    "POST /upload/abc",
  ]);
  assert.equal(said.at(-1), 1);
});
