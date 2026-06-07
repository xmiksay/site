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
npm run serve        # http://localhost:4321/  ->  /preview/index.html
```

`npm run serve` starts a tiny dependency-free dev server with the **design bundle
as document root** (`/` ⇒ `design/`), like the live server. So the app runs at
**`/preview/index.html`** (`/` redirects there) and `/assets/*` serves the runtime
libs and css/js/img. The app's data (`/preview/templates.json`,
`/preview/fixtures.json`) is regenerated on every request, so template and fixture
edits show up on reload. A static server is
required — `fetch` does not work over `file://`.

To assemble the output once (e.g. before building/deploying the server so it
serves the runtime libs under `/assets/`):

```bash
npm run build        # writes design/preview/ and design/assets/{js,img}
```

## Asset URLs — works at any mount point

The page never hard-codes the web root, so it runs wherever the design bundle is
mounted: at the web root (this dev server), or under a prefix when an external
designer tool mounts the whole bundle somewhere (e.g. `design/` ⇒ `/raw/`, so the
page is `/raw/preview/index.html` and assets are `/raw/assets/...`).

- Its **runtime** (minijinja-js js+wasm) and **data** are loaded **relatively**:
  `../assets/js/minijinja_js.js`, `./templates.json`. From `<mount>/preview/` these
  resolve to `<mount>/assets/...` and `<mount>/preview/...` for any `<mount>`.
- The rendered template markup carries absolute `/assets/...` and `/files/...`
  URLs (what the production templates emit). Before handing it to the iframe, the
  page detects its mount base from `location` and rewrites those: `/assets/*` gets
  the prefix, and `/files/*` (real uploads we don't have) collapses to the bundled
  `assets/img/placeholder.svg`.

So the same build works under this dev server and under an external tool's mount
with no configuration.

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
- **Files & images** — the page rewrites every `/files/{hash}` (and
  `/files/{hash}/nahled`) in the rendered markup to the bundled
  `assets/img/placeholder.svg`, since we have no real uploads (this is a
  layout/CSS preview).
- **Page-runtime JS** — `jquery`, `chessboard`, `chess-viewer`, `lightbox`,
  `code-box` load from `/assets` (served from the design bundle) exactly as in
  production, so chess boards and lightboxes work in the preview.

## Files

| Path | Role |
|---|---|
| `index.html` | Browser: init WASM, set loader (with `timeformat` strip), compose `body_html`, render each target into an iframe. Source; copied into `preview/`. |
| `fixtures.mjs` | Default dummy data: one fixture per render target plus directive contexts and body block trees. |
| `build.mjs` | Compiles the app into `preview/` and the runtime libs into `assets/{js,img}`; runs the `--serve` dev server. |
| `placeholder.svg` | Source stand-in image; copied to `assets/img/`, where the page points rewritten `/files/*` URLs. |
| `preview/` | Compiled app (output): `index.html`, `templates.json`, `fixtures.json`. |
| `assets/js/minijinja_js*`, `assets/img/placeholder.svg` | Compiled runtime libs (output), served under `/assets/*`. |
