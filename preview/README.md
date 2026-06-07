# Template preview

Render the **real production MiniJinja templates** (`base.html`, `path_page.html`,
`page_search.html`, `404.html`, `markdown/*.html`) filled with realistic dummy
data — **live in the browser**, with no Rust server, Postgres, or seed data.

Templates are rendered with [minijinja-js](https://github.com/mitsuhiko/minijinja)
(the official WASM bindings). Edit a template under `../design/templates/`, hit
**↻ Reload** (or refresh), and see the change. This complements the static
component mockups; here you preview the templates the server actually uses.

This tool lives **outside** the `design/` bundle on purpose, so it is never
compiled into the server binary. It reads the bundle's templates and assets from
the sibling `../design/` folder.

## Run

```bash
cd preview
npm install
npm run serve        # http://localhost:4321
```

`npm run serve` starts a tiny dependency-free dev server. `templates.json` and
`fixtures.json` are regenerated on every request, so template and fixture edits
show up on reload. A static server is required — `fetch` does not work over
`file://`.

To produce a static `dist/` (e.g. to inspect the assembled manifest):

```bash
npm run build
```

## Previewing an override (DESIGN_DIR)

The template loader mirrors the Rust `DesignStore` (`src/design.rs`): an override
folder is preferred, falling back to the baked `design/` bundle. Point the
preview at an override the same way the server does:

```bash
DESIGN_DIR=/path/to/override npm run serve
# or
node build.mjs --serve --design-dir=/path/to/override
```

Each template name resolves to `<override>/templates/<name>` when present, else
`design/templates/<name>`. Static assets under `/static/{css,js,img}` resolve the
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
- **Files & images** — `/files/{hash}` and `/files/{hash}/nahled` are served as a
  bundled placeholder image.
- **Page-runtime JS** — `jquery`, `chessboard`, `chess-viewer`, `lightbox`,
  `code-box` load from `/static` (served from the design bundle) exactly as in
  production, so chess boards and lightboxes work in the preview.

## Files

| File | Role |
|---|---|
| `index.html` | Browser: init WASM, set loader (with `timeformat` strip), compose `body_html`, render each target into an iframe. |
| `fixtures.mjs` | Default dummy data: one fixture per render target plus directive contexts and body block trees. |
| `build.mjs` | Assembles `dist/` (template manifest + fixtures + WASM) and runs the `--serve` dev server. |
| `dist/` | Generated (gitignored): `templates.json`, `fixtures.json`, copied minijinja-js WASM/JS. |
