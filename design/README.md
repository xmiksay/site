# Template preview

Render the **real production MiniJinja templates** (`base.html`, `path_page.html`,
`page_search.html`, `404.html`, `markdown/*.html`) filled with realistic dummy
data — **live in the browser**, with no Rust server, Postgres, or seed data.

Templates are rendered with [minijinja-js](https://github.com/mitsuhiko/minijinja)
(the official WASM bindings). Edit a template under `templates/`, hit **↻ Reload**
(or refresh), and see the change. This complements the static component mockups;
here you preview the templates the server actually uses.

## Bundle layout

The design bundle is split by how the server handles each part:

- `templates/` — rendered by the template engine.
- `assets/` — served statically under `/assets/*` (`assets/css`, `assets/js`,
  `assets/img`).

The preview tooling lives at the bundle root (`build.mjs`, `fixtures.mjs`,
`index.html`), and its build output lands in **`assets/preview/`**. Since the
real server serves everything under `assets/` at `/assets/*`, it serves the
preview at **`/assets/preview/index.html`** with no extra wiring — the output is
plain helper artifacts, not design.

## Run

```bash
cd design
npm install
npm run serve        # http://localhost:4321  (redirects to /assets/preview/index.html)
```

`npm run serve` starts a tiny dependency-free dev server mounted at the same
`/assets/preview` prefix the real server uses, so the base path matches. The
manifest (`templates.json`) and `fixtures.json` are regenerated on every request,
so template and fixture edits show up on reload. A static server is required —
`fetch` does not work over `file://`.

To assemble the output once (e.g. before building/deploying the server so it can
serve `/assets/preview/`):

```bash
npm run build        # writes design/assets/preview/
```

## Base path — why everything is relative

The output is mounted under `/assets/preview/`, not the web root. So `index.html`
references its own assets **relatively** (`./minijinja_js.js`, `./templates.json`,
…), never with a leading slash — it is base-path agnostic and runs the same off
the dev server or a live site. The rendered template markup inside the iframe
keeps its absolute `/assets/...` and `/files/...` URLs; both servers resolve those
from the web root.

## Previewing an override (DESIGN_DIR)

The template loader mirrors the Rust `DesignStore` (`src/design.rs`): an override
folder is preferred, falling back to this bundle. Point the preview at an override
the same way the server does:

```bash
DESIGN_DIR=/path/to/override npm run serve
# or
node build.mjs --serve --design-dir=/path/to/override
```

Each template name resolves to `<override>/templates/<name>` when present, else
`design/templates/<name>`. Static assets under `/assets/{css,js,img}` resolve the
same way.

## What's faked, and why

- **`timeformat` filter** — minijinja-js cannot register custom JS filters, so the
  one custom filter (`src/state.rs`) is stripped out of the template source as it
  loads; fixtures carry pre-formatted date strings. Single shim point; the
  on-disk templates are untouched.
- **Markdown directives** — the Rust directive parser (`expand_directives` in
  `src/markdown.rs`) is **not** ported. Instead `fixtures.mjs` supplies the
  pre-expanded directive contexts, and the markdown directive templates
  (`markdown/page.html`, `gallery.html`, …) are rendered through minijinja-js and
  concatenated into `body_html`. A `<page>` transclude whose inner content itself
  contains a rendered directive is encoded as a nested block tree — the loopback
  expressed as data.
- **Files & images** — under the dev server, `/files/{hash}` and
  `/files/{hash}/nahled` are served as a bundled placeholder image. On a live
  site the dummy hashes won't resolve (this is a layout/CSS preview).
- **Page-runtime JS** — `jquery`, `chessboard`, `chess-viewer`, `lightbox`,
  `code-box` load from `/assets` (served from the design bundle) exactly as in
  production, so chess boards and lightboxes work in the preview.

## Files

| Path | Role |
|---|---|
| `index.html` | Browser: init WASM, set loader (with `timeformat` strip), compose `body_html`, render each target into an iframe. Source; copied into `assets/preview/`. |
| `fixtures.mjs` | Default dummy data: one fixture per render target plus directive contexts and body block trees. |
| `build.mjs` | Assembles `assets/preview/` (manifest + fixtures + WASM + index/placeholder) and runs the `--serve` dev server. |
| `placeholder.svg` | Stand-in image for `/files/*` in the dev server. |
| `assets/preview/` | Build output, served at `/assets/preview/`: `templates.json`, `fixtures.json`, `index.html`, `placeholder.svg`, copied minijinja-js WASM/JS. |
