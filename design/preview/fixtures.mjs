// Default dummy data for the template preview.
//
// One fixture per render target, matching the structs the Rust public handlers
// serialize (see src/routes/public/{mod,pages,search}.rs and src/routes/mod.rs).
// Dates are PRE-FORMATTED strings: the `timeformat` filter is stripped out in
// the browser loader (minijinja-js cannot register custom JS filters), so
// `{{ page.modified_at }}` renders these verbatim.
//
// `body` is a recursive block tree that the browser composes into `body_html`
// by rendering the markdown/*.html directive templates and concatenating the
// results — this is the "directive loopback" expressed as data. Block shapes:
//   { type: "prose",     html }                       — raw HTML passthrough
//   { type: "directive", name, ctx }                  — render markdown/<name>.html
//   { type: "page",      path, body: [...nested] }    — <page> transclude (loopback)
// The directive `ctx` shapes mirror the context!{} call sites in src/markdown.rs.

// Formatted the way `modified_at|timeformat("%d. %m. %Y")` would render it.
const DATE = "07. 06. 2026";

// A 64-hex placeholder file hash. The dev server maps /files/<hash> and
// /files/<hash>/nahled to a bundled placeholder image, so directive output that
// references files renders with a visible stand-in.
const HASH = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

// Shared navigation. base.html renders `menu_tree`; `menu_list` is passed by the
// handlers too, so it is included for contract completeness.
const menuTree = [
  {
    path: "/notes",
    label: "Notes",
    children: [
      { path: "/notes/rust", label: "Rust", children: [] },
      { path: "/notes/chess", label: "Chess", children: [] },
    ],
  },
  {
    path: "/projects",
    label: "Projects",
    children: [{ path: "/projects/site", label: "Site", children: [] }],
  },
  { path: "/about", label: "About", children: [] },
];

const menuList = [
  { path: "/notes", label: "Notes" },
  { path: "/notes/rust", label: "Rust" },
  { path: "/notes/chess", label: "Chess" },
  { path: "/projects", label: "Projects" },
  { path: "/projects/site", label: "Site" },
  { path: "/about", label: "About" },
];

// A hand-written stand-in SVG for the mermaid directive (the Rust mermaid->SVG
// renderer is server-side only; fixtures supply pre-rendered output).
const MERMAID_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="70" viewBox="0 0 240 70">' +
  '<rect x="8" y="20" width="86" height="30" rx="5" fill="#e3f2fd" stroke="#1565c0"/>' +
  '<text x="51" y="40" text-anchor="middle" font-size="13" font-family="sans-serif">Start</text>' +
  '<line x1="94" y1="35" x2="142" y2="35" stroke="#555" stroke-width="1.5"/>' +
  '<polygon points="142,31 150,35 142,39" fill="#555"/>' +
  '<rect x="150" y="20" width="82" height="30" rx="5" fill="#e8f5e9" stroke="#2e7d32"/>' +
  '<text x="191" y="40" text-anchor="middle" font-size="13" font-family="sans-serif">End</text>' +
  "</svg>";

// Body of the page-detail fixture: prose interleaved with every directive, and a
// <page> transclude whose own body contains a rendered gallery (the loopback).
const pageDetailBody = [
  {
    type: "prose",
    html:
      "<h2>Ownership and borrowing</h2>" +
      '<p>An <a href="notes/rust/lifetimes.md">internal link</a> and ' +
      'an <a href="https://doc.rust-lang.org">external one</a>. Inline ' +
      "<code>let x = 5;</code> renders too.</p>" +
      '<pre class="code-block" data-lang="rust"><code>fn main() {\n' +
      '    println!("hello");\n}</code></pre>',
  },
  {
    type: "directive",
    name: "img",
    ctx: { hash: HASH, title: "Architecture", alt: "Architecture diagram" },
  },
  {
    type: "directive",
    name: "gallery",
    ctx: {
      id: 3,
      title: "Holiday 2024",
      items: [
        { hash: HASH, title: "Beach" },
        { hash: HASH, title: "Sunset" },
        { hash: HASH, title: "Mountains" },
      ],
    },
  },
  {
    type: "directive",
    name: "fen",
    ctx: {
      fen: "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
      size_class: "",
    },
  },
  {
    type: "directive",
    name: "pgn",
    ctx: {
      pgn: "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7",
      size_class: " size-lg",
      move: "6",
    },
  },
  {
    type: "directive",
    name: "json",
    ctx: {
      kind: "table",
      columns: ["Name", "Score"],
      rows: [
        ["Alice", "42"],
        ["Bob", "17"],
      ],
    },
  },
  {
    type: "directive",
    name: "mermaid",
    ctx: { svg: MERMAID_SVG, source: "graph LR; Start --> End", size_class: "" },
  },
  {
    type: "directive",
    name: "file",
    ctx: { hash: HASH, title: "spec.pdf", description: "Download the spec (PDF)" },
  },
  { type: "prose", html: "<h3>Transcluded page</h3>" },
  {
    type: "page",
    path: "notes/shared/snippet",
    body: [
      { type: "prose", html: "<p>This block is transcluded from another page.</p>" },
      {
        type: "directive",
        name: "gallery",
        ctx: {
          id: 9,
          title: "Nested gallery (loopback)",
          items: [
            { hash: HASH, title: "Nested A" },
            { hash: HASH, title: "Nested B" },
          ],
        },
      },
    ],
  },
];

const menuPageBody = [
  {
    type: "prose",
    html:
      "<h1>Welcome</h1>" +
      "<p>This is a menu landing page. It renders through " +
      "<code>path_page.html</code> with no <code>page</code> object — just " +
      "<code>body_html</code> and an edit bar for logged-in users.</p>",
  },
];

// pages[] entries for the search fixture (PageView shape; only path/summary/
// modified_at are used by page_search.html).
const searchPages = [
  {
    id: 1,
    path: "notes/rust/ownership",
    summary: "Notes on ownership and borrowing",
    tag_ids: [1],
    private: false,
    created_at: DATE,
    modified_at: DATE,
  },
  {
    id: 2,
    path: "notes/rust/lifetimes",
    summary: "How lifetimes work",
    tag_ids: [1],
    private: false,
    created_at: DATE,
    modified_at: DATE,
  },
  {
    id: 3,
    path: "notes/rust/traits",
    summary: null,
    tag_ids: [1],
    private: false,
    created_at: DATE,
    modified_at: DATE,
  },
];

export default {
  // Targets keyed by id. Each carries the template name, a label for the UI, the
  // render context, and (when the template shows body_html) a `body` block tree.
  targets: {
    page: {
      label: "Page detail",
      template: "path_page.html",
      body: pageDetailBody,
      context: {
        page: {
          id: 12,
          path: "notes/rust/ownership",
          summary: "Notes on ownership and borrowing",
          tag_ids: [1, 2],
          private: false,
          created_at: DATE,
          modified_at: DATE,
        },
        breadcrumbs: [
          { label: "notes", href: "/notes" },
          { label: "rust", href: "/notes/rust" },
          { label: "ownership", href: "/notes/rust/ownership" },
        ],
        tags: [
          { id: 1, name: "Rust", description: "The Rust programming language" },
          { id: 2, name: "Notes", description: null },
        ],
        menu_list: menuList,
        menu_tree: menuTree,
        logged_in: true,
      },
    },

    menu: {
      label: "Menu page",
      template: "path_page.html",
      body: menuPageBody,
      context: {
        menu_id: 5,
        menu_list: menuList,
        menu_tree: menuTree,
        logged_in: true,
      },
    },

    search: {
      label: "Search",
      template: "page_search.html",
      context: {
        q: "rust",
        tag: null,
        tag_name: "",
        path_prefix: "",
        pages: searchPages,
        total: 42,
        limit: 20,
        offset: 0,
        prev_offset: null,
        next_offset: 20,
        menu_list: menuList,
        menu_tree: menuTree,
        logged_in: false,
      },
    },

    notFound: {
      label: "404",
      template: "404.html",
      context: {
        menu_list: menuList,
        menu_tree: menuTree,
        logged_in: false,
      },
    },
  },
};
