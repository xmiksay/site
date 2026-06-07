# Template preview

Render the **real production MiniJinja templates** (`base.html`, `path_page.html`,
`page_search.html`, `404.html`, `markdown/*.html`) filled with realistic dummy
data into **standalone HTML files** — no Rust server, Postgres, or seed data.

Rendering happens in Node with [minijinja-js](https://github.com/mitsuhiko/minijinja)
(the official WASM bindings). There is **no router and no in-browser WASM**: the
build writes one ready-to-open page per render target. Edit a template, rebuild
(or just refresh under the dev server), and see the change.

## Bundle layout

The design bundle is split by how the server handles each part:

- `templates/` — rendered by the template engine.
- `assets/` — served statically under `/assets/*` (`assets/css`, `assets/js`,
  `assets/img`).

The preview tooling lives at the bundle root (`build.mjs`, `fixtures.mjs`,
`placeholder.svg`). The build renders each target to its own file in
**`preview/`**:

```
preview/index.html    a static link list (no JS)
preview/page.html     path_page.html with a page fixture
preview/menu.html     path_page.html as a menu landing (no page)
preview/search.html   page_search.html
preview/404.html      404.html
```

## Run

```bash
cd design
npm install
npm run serve        # http://localhost:4321/  ->  /preview/index.html
```

`npm run serve` starts a tiny dependency-free dev server with the **design bundle
as document root** (`/` ⇒ `design/`), like the live server. The preview pages
**re-render on every request**, so template and fixture edits show up on reload.
A static server is required — `fetch`/module loading does not work over `file://`.

To just write the files once (e.g. before building/deploying the server):

```bash
npm run build        # writes design/preview/
```

## Mount-agnostic asset URLs

Each rendered page lives in `preview/` and must work wherever the bundle is
mounted: at the web root, or under a prefix when an external tool mounts the
whole bundle somewhere (e.g. `design/` ⇒ `/raw/`, so a page is
`/raw/preview/page.html`). So the build rewrites the absolute URLs the production
templates emit into paths **relative to `preview/`**:

- `/assets/*` → `../assets/*` — resolves to `<mount>/assets/*` for any mount.
- `/files/*` (real uploads we don't have) → `../assets/img/placeholder.svg`.

The page-runtime JS (`jquery`, `chessboard`, `chess-viewer`, `lightbox`,
`code-box`) loads from `../assets/js` exactly as in production, so chess boards and
lightboxes work in the preview.

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
`design/templates/<name>`.

## What's faked, and why

- **`timeformat` filter** — minijinja-js cannot register custom JS filters, so the
  one custom filter (`src/state.rs`) is stripped out of the template source as it
  loads; fixtures carry pre-formatted date strings. Single shim point; the
  on-disk templates are untouched.
- **Markdown directives** — the Rust directive parser (`expand_directives` in
  `src/markdown.rs`) is **not** ported. Instead `fixtures.mjs` supplies the
  pre-expanded directive contexts, and the markdown directive templates
  (`markdown/page.html`, `gallery.html`, …) are rendered and concatenated into
  `body_html`. A `<page>` transclude whose inner content itself contains a
  rendered directive is encoded as a nested block tree — the loopback as data.
- **Files & images** — `/files/*` is rewritten to the bundled
  `assets/img/placeholder.svg`, since we have no real uploads (layout/CSS preview).

## Files

| Path | Role |
|---|---|
| `build.mjs` | Renders each fixture target to `preview/<file>.html` (+ a static `index.html`) and runs the `--serve` dev server. |
| `fixtures.mjs` | Default dummy data: one fixture per render target (with its output `file`) plus directive contexts and body block trees. |
| `placeholder.svg` | Source stand-in image; copied to `assets/img/`, where rewritten `/files/*` URLs point. |
| `preview/` | Build output: `index.html`, `page.html`, `menu.html`, `search.html`, `404.html`. |
