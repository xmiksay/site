// Build / dev-server for the browser-live template preview.
//
//   node build.mjs            assemble dist/ (templates.json, fixtures.json, WASM)
//   node build.mjs --serve    start a static dev server (templates regenerate live)
//
// Template resolution mirrors the Rust DesignStore (src/design.rs): an optional
// override folder (DESIGN_DIR / --design-dir) is preferred, falling back to the
// baked design bundle (this folder's parent). `fetch` does not work over
// file://, so a static server is required either way.

import { createServer } from "node:http";
import { readdir, readFile, mkdir, copyFile, stat } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve, extname } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const BAKED_DIR = resolve(HERE, ".."); // the design/ bundle root
const DIST_DIR = join(HERE, "dist");
const RUNTIME_SRC = join(HERE, "node_modules", "minijinja-js", "dist", "web");
const RUNTIME_FILES = ["minijinja_js.js", "minijinja_js_bg.wasm"];

// --- argument / override resolution ----------------------------------------

function overrideDir() {
  const flag = process.argv.find((a) => a.startsWith("--design-dir="));
  const value = flag ? flag.slice("--design-dir=".length) : process.env.DESIGN_DIR;
  return value && value.length ? resolve(value) : null;
}

// Resolve a bundle-relative path (e.g. "css/style.css", "templates/base.html")
// against the override folder, falling back to the baked bundle. Returns an
// absolute path or null.
function resolveAsset(relPath, override) {
  if (override) {
    const candidate = join(override, relPath);
    if (existsSync(candidate)) return candidate;
  }
  const baked = join(BAKED_DIR, relPath);
  return existsSync(baked) ? baked : null;
}

// --- template manifest ------------------------------------------------------

async function listTemplates(baseDir) {
  const root = join(baseDir, "templates");
  if (!existsSync(root)) return [];
  const out = [];
  async function walk(dir, prefix) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await walk(join(dir, entry.name), rel);
      else if (entry.name.endsWith(".html")) out.push(rel);
    }
  }
  await walk(root, "");
  return out;
}

// Build { "<name>": "<raw source>" } preferring the override, falling back to
// baked — the union of template names across both layers.
async function buildManifest(override) {
  const names = new Set([
    ...(await listTemplates(BAKED_DIR)),
    ...(override ? await listTemplates(override) : []),
  ]);
  const manifest = {};
  for (const name of [...names].sort()) {
    const path = resolveAsset(`templates/${name}`, override);
    if (path) manifest[name] = await readFile(path, "utf8");
  }
  return manifest;
}

async function loadFixtures() {
  const mod = await import(`./fixtures.mjs?t=${Date.now()}`);
  return mod.default;
}

// --- runtime (WASM) copy ----------------------------------------------------

async function copyRuntime() {
  if (!existsSync(RUNTIME_SRC)) {
    throw new Error(
      `minijinja-js not found at ${RUNTIME_SRC}. Run \`npm install\` first.`,
    );
  }
  await mkdir(DIST_DIR, { recursive: true });
  for (const file of RUNTIME_FILES) {
    await copyFile(join(RUNTIME_SRC, file), join(DIST_DIR, file));
  }
}

// --- build mode -------------------------------------------------------------

async function build() {
  const override = overrideDir();
  await mkdir(DIST_DIR, { recursive: true });
  const manifest = await buildManifest(override);
  const fixtures = await loadFixtures();
  await writeJson(join(DIST_DIR, "templates.json"), manifest);
  await writeJson(join(DIST_DIR, "fixtures.json"), fixtures);
  await copyRuntime();
  console.log(
    `built dist/ — ${Object.keys(manifest).length} template(s)` +
      (override ? `, override: ${override}` : ", baked bundle only"),
  );
}

async function writeJson(path, value) {
  const { writeFile } = await import("node:fs/promises");
  await writeFile(path, JSON.stringify(value, null, 2));
}

// --- serve mode -------------------------------------------------------------

const MIME = {
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".css": "text/css",
  ".html": "text/html; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".ico": "image/x-icon",
};

const mimeFor = (path) => MIME[extname(path).toLowerCase()] || "application/octet-stream";

function send(res, status, body, type) {
  res.writeHead(status, {
    "content-type": type || "text/plain; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(body);
}

async function sendFile(res, path) {
  try {
    const data = await readFile(path);
    send(res, 200, data, mimeFor(path));
  } catch {
    send(res, 404, "Not Found");
  }
}

async function serve() {
  const override = overrideDir();
  const port = Number(process.env.PORT) || 4321;
  await copyRuntime(); // ensure /dist/minijinja_js.* are present
  const placeholder = join(HERE, "placeholder.svg");

  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://localhost");
      const path = decodeURIComponent(url.pathname);

      // Templates & fixtures regenerate per request so edits show on refresh.
      if (path === "/dist/templates.json") {
        return send(res, 200, JSON.stringify(await buildManifest(override)), MIME[".json"]);
      }
      if (path === "/dist/fixtures.json") {
        return send(res, 200, JSON.stringify(await loadFixtures()), MIME[".json"]);
      }
      // WASM runtime, copied into dist/.
      if (path.startsWith("/dist/")) {
        return sendFile(res, join(DIST_DIR, path.slice("/dist/".length)));
      }
      // /static/{css,js,img}/* → override → baked bundle (mirrors the /static route).
      if (path.startsWith("/static/")) {
        const resolved = resolveAsset(path.slice("/static/".length), override);
        return resolved ? sendFile(res, resolved) : send(res, 404, "Not Found");
      }
      // /files/{hash} and /files/{hash}/nahled → bundled placeholder image.
      if (path.startsWith("/files/")) {
        return sendFile(res, placeholder);
      }
      if (path === "/" || path === "/index.html") {
        return sendFile(res, join(HERE, "index.html"));
      }
      send(res, 404, "Not Found");
    } catch (err) {
      send(res, 500, String(err && err.stack ? err.stack : err));
    }
  });

  server.listen(port, () => {
    console.log(`preview server on http://localhost:${port}`);
    console.log(override ? `override: ${override}` : "baked bundle only");
  });
}

// --- entry ------------------------------------------------------------------

const isServe = process.argv.includes("--serve");
try {
  await (isServe ? serve() : build());
} catch (err) {
  console.error(err.message || err);
  process.exit(1);
}
