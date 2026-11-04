// What app/ expects of a browser, so its modules load in Node. Unknown properties answer with a
// stand-in that takes anything, so drawing and layout do nothing; what a test reads is real.
// A property of a stand-in: what was set on it, or a new stand-in that it then keeps.
const kept = (t, k) => (k in t ? t[k] : (t[k] = anything()));
const anything = () =>
  new Proxy(function () {}, {
    get: (t, k) => (k === Symbol.toPrimitive ? () => 0 : kept(t, k)),
    set: (t, k, v) => {
      t[k] = v;
      return true;
    },
    apply: () => anything(),
    construct: () => anything(),
  });

// An element: the feed and chat are read back through `children` and `innerHTML`.
class FakeElement {
  children = [];
  style = { display: "", setProperty() {} };
  classes = new Set();
  classList = {
    add: c => this.classes.add(c),
    remove: c => this.classes.delete(c),
    toggle() {},
    contains: c => this.classes.has(c),
  };
  innerHTML = "";
  textContent = "";
  className = "";
  get lastChild() {
    return this.children.at(-1);
  }
  prepend(el) {
    el.parent = this;
    this.children.unshift(el);
  }
  remove() {
    const c = this.parent.children;
    c.splice(c.indexOf(this), 1);
  }
  replaceChildren() {
    this.children = [];
  }
}
const element = () => new Proxy(new FakeElement(), { get: kept });

const byId = new Map();
globalThis.window = globalThis;
globalThis.document = {
  baseURI: "http://stv.test/",
  getElementById: id => {
    if (!byId.has(id)) byId.set(id, element());
    return byId.get(id);
  },
  createElement: () => element(),
  addEventListener() {},
  querySelectorAll: () => [],
};
globalThis.location ??= { pathname: "/", search: "" };
globalThis.parent = globalThis;
globalThis.innerHeight = 1080;
globalThis.innerWidth = 1920;
globalThis.addEventListener = () => {};
globalThis.HTMLElement = class {};
globalThis.customElements = { define() {}, get: () => undefined };
globalThis.Image = class {
  complete = false;
  naturalWidth = 0;
};
globalThis.ResizeObserver = class {
  observe() {}
};
globalThis.EventSource = class {
  close() {}
};
globalThis.localStorage = { getItem: () => null, setItem() {} };
globalThis.requestAnimationFrame = () => 0;
globalThis.Path2D = class {
  addPath() {}
  rect() {}
  moveTo() {}
  lineTo() {}
  closePath() {}
};
globalThis.fetch = async () => ({
  ok: true,
  json: async () => ({}),
  text: async () => ".killicon-quake_rl { width:50px; height:20px; background-position:-10px -30px; }",
});
// A stream's status clock must not keep a test's process alive.
const { setInterval: nodeSetInterval } = globalThis;
globalThis.setInterval = (fn, ms) => nodeSetInterval(fn, ms).unref();
