import type { DesignFile } from '../types'

export const DESIGN_ROOTS = ['templates', 'assets', 'mdcast']

const TEXT_EXTENSIONS = new Set([
  'html', 'htm', 'css', 'js', 'mjs', 'json', 'toml', 'typ', 'txt', 'svg', 'md', 'xml', 'yml', 'yaml',
])
export const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'svg', 'ico'])

function extension(path: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1)
  const dot = name.lastIndexOf('.')
  return dot <= 0 ? '' : name.slice(dot + 1).toLowerCase()
}

export function isTextPath(path: string): boolean {
  return TEXT_EXTENSIONS.has(extension(path))
}

export function isImagePath(path: string): boolean {
  return IMAGE_EXTENSIONS.has(extension(path))
}

export function parentFolder(path: string): string {
  const slash = path.lastIndexOf('/')
  return slash === -1 ? '' : path.slice(0, slash)
}

/** Which view a draft file read comes from (`GET /api/design/draft/{path}?source=`). */
export type DesignSource = 'draft' | 'published' | 'baked'

export function designFileUrl(path: string, source: DesignSource = 'draft'): string {
  const encoded = path.split('/').map(encodeURIComponent).join('/')
  return `/api/design/draft/${encoded}${source === 'draft' ? '' : `?source=${source}`}`
}

/** A site-relative URL for the preview iframe from what the admin typed, never
 *  leaving `origin`: browsers drop tabs/newlines and read `\` as `/`, so
 *  `/\t/host` or `/\host` would otherwise become a protocol-relative URL. */
export function previewPath(input: string, origin: string = location.origin): string {
  const path = `/${input.replace(/[\t\n\r]/g, '').trim().replace(/^[/\\]+/, '')}`
  try {
    const url = new URL(path, origin)
    return url.origin === origin ? `${url.pathname}${url.search}${url.hash}` : '/'
  } catch {
    return '/'
  }
}

export const FONT_EXTENSIONS = new Set(['woff', 'woff2', 'ttf', 'otf', 'eot'])

/** Where an uploaded file lands by default: images under `assets/img/`, fonts
 *  under `assets/fonts/`, anything else in the folder picked in the tree. */
export function defaultUploadPath(name: string, folder: string): string {
  const ext = extension(name)
  if (FONT_EXTENSIONS.has(ext)) return `assets/fonts/${name}`
  if (IMAGE_EXTENSIONS.has(ext)) return `assets/img/${name}`
  return folder ? `${folder}/${name}` : name
}

/** Returns an error message, or null when `path` is an acceptable override target. */
export function validateDesignPath(path: string): string | null {
  const segments = path.split('/')
  if (segments.length < 2 || !DESIGN_ROOTS.includes(segments[0])) {
    return 'Path must start with templates/, assets/ or mdcast/'
  }
  if (segments.some((s) => s === '' || s === '.' || s === '..')) {
    return 'Path must not contain empty, "." or ".." segments'
  }
  return null
}

export type DesignFileStatus = 'baked' | 'overridden' | 'override-only'

export function fileStatus(file: DesignFile): DesignFileStatus {
  if (!file.overridden) return 'baked'
  return file.baked ? 'overridden' : 'override-only'
}

export interface DesignTreeRow {
  kind: 'folder' | 'file'
  name: string
  path: string
  depth: number
  file?: DesignFile
}

interface TreeNode {
  name: string
  path: string
  children: Map<string, TreeNode>
  file?: DesignFile
}

function buildTree(files: DesignFile[]): TreeNode {
  const root: TreeNode = { name: '', path: '', children: new Map() }
  for (const file of files) {
    let node = root
    for (const segment of file.path.split('/')) {
      let child = node.children.get(segment)
      if (!child) {
        const path = node.path ? `${node.path}/${segment}` : segment
        child = { name: segment, path, children: new Map() }
        node.children.set(segment, child)
      }
      node = child
    }
    node.file = file
  }
  return root
}

/** Flattens `files` into display rows (folders first, then files, by name),
 *  descending only into folders listed in `expanded`. */
export function visibleRows(files: DesignFile[], expanded: Set<string>): DesignTreeRow[] {
  const rows: DesignTreeRow[] = []
  const walk = (node: TreeNode, depth: number) => {
    const children = [...node.children.values()]
    const folders = children.filter((c) => !c.file).sort((a, b) => a.name.localeCompare(b.name))
    const leaves = children.filter((c) => c.file).sort((a, b) => a.name.localeCompare(b.name))
    for (const folder of folders) {
      rows.push({ kind: 'folder', name: folder.name, path: folder.path, depth })
      if (expanded.has(folder.path)) walk(folder, depth + 1)
    }
    for (const leaf of leaves) {
      rows.push({ kind: 'file', name: leaf.name, path: leaf.path, depth, file: leaf.file })
    }
  }
  walk(buildTree(files), 0)
  return rows
}

/** Every ancestor folder of `path`, outermost first. */
export function ancestorFolders(path: string): string[] {
  const segments = path.split('/').slice(0, -1)
  return segments.map((_, i) => segments.slice(0, i + 1).join('/'))
}
