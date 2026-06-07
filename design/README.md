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
`index.html`). The compiler writes two kinds of output:

- the **preview app** (the page + its data) → **`preview/`**:
  `index.html`, `templates.json`, `fixtures.json`.
- the **runtime libraries** it needs to run → the served **`assets/`** folders:
  the minijinja-js runtime (`minijinja_js.js`, `minijinja_js_bg.wasm`) →
  `assets/js/`, the placeholder image → `assets/img/`.

So compiled libraries land where every other static resource lives (served at
`/assets/*`), and the page itself stays a plain helper in `preview/`.

## Run

```bash
cd design
npm install
npm run serve        # http://localhost:4321
```

`npm run serve` starts a tiny dependency-free dev server: it serves the page from
`preview/` at the web root, the runtime/static assets at `/assets/*`, and a
placeholder for `/files/*`. The manifest (`templates.json`) and `fixtures.json`
are regenerated on every request, so template and fixture edits show up on
reload. A static server is required — `fetch` does not work over `file://`.

To assemble the output once (e.g. before building/deploying the server so it
serves the runtime libs under `/assets/`):

```bash
npm run build        # writes design/preview/ and design/assets/{js,img}
```

## Asset URLs

The page loads its **runtime** with absolute `/assets/js/...` URLs — that's where
the compiler places the libs and where the real `/assets` route serves them. It
fetches its own **data** relatively (`./templates.json`, `./fixtures.json`), which
sit beside `index.html` in `preview/`. The rendered template markup inside the
iframe keeps its absolute `/assets/...` and `/files/...` URLs, resolved from the
web root.

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
| `index.html` | Browser: init WASM, set loader (with `timeformat` strip), compose `body_html`, render each target into an iframe. Source; copied into `preview/`. |
| `fixtures.mjs` | Default dummy data: one fixture per render target plus directive contexts and body block trees. |
| `build.mjs` | Compiles the app into `preview/` and the runtime libs into `assets/{js,img}`; runs the `--serve` dev server. |
| `placeholder.svg` | Source stand-in image; copied to `assets/img/` and served for `/files/*` in the dev server. |
| `preview/` | Compiled app (output): `index.html`, `templates.json`, `fixtures.json`. |
| `assets/js/minijinja_js*`, `assets/img/placeholder.svg` | Compiled runtime libs (output), served under `/assets/*`. |
