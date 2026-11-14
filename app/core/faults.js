// What goes wrong on the page is told to the server (POST fault), which keeps track of it. Loaded
// before the viewer, so its failing to start is told too.

// The most told by one page: one that fails every frame says it once.
let left = 5;

/** @param {{ name?: string; message?: string; stack?: string } | null | undefined} e */
function tell(e) {
  const stack = String(e?.stack ?? "");
  // "Script error." is another origin's, which says no more; an extension's is not ours.
  if (!e?.message || e.message === "Script error." || stack.includes("extension://") || left-- <= 0) return;
  const body = JSON.stringify({
    kind: e.name ?? "Error",
    message: String(e.message).slice(0, 2000),
    stack: stack.slice(0, 8000),
    page: location.pathname,
    build: /\/app\/([^/]+)\//.exec(import.meta.url)?.[1] ?? "",
  });
  fetch(new URL("fault", document.baseURI), { method: "POST", keepalive: true, body }).catch(() => {});
}

addEventListener("error", e => tell(e.error ?? { message: e.message, stack: `@${e.filename}:${e.lineno}:${e.colno}` }));
addEventListener("unhandledrejection", e =>
  tell(e.reason instanceof Error ? e.reason : { name: "UnhandledRejection", message: String(e.reason) }),
);
