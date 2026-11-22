// Reports errors on the page to the server (POST fault), which keeps track of them. Loaded before
// the viewer, so a viewer that fails to start is reported too.

// The most faults one page reports, so a page that fails every frame does not flood the server.
let left = 5;
// The trace of the last request the server failed (`x-trace-id`), sent with a fault.
let trace = "";

const fetched = globalThis.fetch.bind(globalThis);
/** @type {typeof fetch} */
globalThis.fetch = async (...args) => {
  const r = await fetched(...args);
  if (!r.ok) trace = r.headers.get("x-trace-id") ?? trace;
  return r;
};

/** @param {{ name?: string; message?: string; stack?: string } | null | undefined} e */
function tell(e) {
  const stack = String(e?.stack ?? "");
  // "Script error." comes from another origin's script and carries no detail; an extension's error is not ours.
  if (!e?.message || e.message === "Script error." || stack.includes("extension://") || left-- <= 0) return;
  const body = JSON.stringify({
    kind: e.name ?? "Error",
    message: String(e.message).slice(0, 2000),
    stack: stack.slice(0, 8000),
    page: location.pathname,
    build: /\/app\/([^/]+)\//.exec(import.meta.url)?.[1] ?? "",
    trace,
  });
  fetched(new URL("fault", document.baseURI), { method: "POST", keepalive: true, body }).catch(() => {});
}

addEventListener("error", e => tell(e.error ?? { message: e.message, stack: `@${e.filename}:${e.lineno}:${e.colno}` }));
addEventListener("unhandledrejection", e =>
  tell(e.reason instanceof Error ? e.reason : { name: "UnhandledRejection", message: String(e.reason) }),
);
