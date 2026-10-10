import { describe, it, expect } from 'vitest'
import {
  ancestorFolders,
  designFileUrl,
  fileStatus,
  isImagePath,
  isTextPath,
  parentFolder,
  validateDesignPath,
  visibleRows,
} from './designPaths'
import type { DesignFile } from '../types'

function f(path: string, baked = true, overridden = false): DesignFile {
  return { path, baked, overridden, size: 1 }
}

describe('validateDesignPath', () => {
  it.each(['templates/base.html', 'assets/css/style.css', 'mdcast/typst/layouts/pdf/a.typ'])(
    'accepts %s',
    (p) => expect(validateDesignPath(p)).toBeNull(),
  )

  it.each(['base.html', 'other/x.css', 'templates', 'templates/', '/templates/x'])(
    'rejects bad root %s',
    (p) => expect(validateDesignPath(p)).not.toBeNull(),
  )

  it.each(['templates//x.html', 'assets/./x.css', 'assets/../x.css', 'assets/css/'])(
    'rejects bad segments in %s',
    (p) => expect(validateDesignPath(p)).toMatch(/segments/),
  )
})

describe('path helpers', () => {
  it('classifies text and image extensions', () => {
    expect(isTextPath('templates/base.HTML')).toBe(true)
    expect(isTextPath('mdcast/brand.toml')).toBe(true)
    expect(isTextPath('assets/img/logo.png')).toBe(false)
    expect(isTextPath('assets/.hidden')).toBe(false)
    expect(isImagePath('assets/img/logo.png')).toBe(true)
    expect(isImagePath('assets/img/logo.svg')).toBe(true)
  })

  it('encodes each segment but keeps slashes', () => {
    expect(designFileUrl('assets/img/a b#.png')).toBe('/api/design/draft/assets/img/a%20b%23.png')
    expect(designFileUrl('templates/x.html', true)).toBe(
      '/api/design/draft/templates/x.html?source=baked',
    )
  })

  it('derives parent and ancestor folders', () => {
    expect(parentFolder('assets/css/style.css')).toBe('assets/css')
    expect(ancestorFolders('assets/css/style.css')).toEqual(['assets', 'assets/css'])
  })

  it('maps baked/overridden flags to a status', () => {
    expect(fileStatus(f('a', true, false))).toBe('baked')
    expect(fileStatus(f('a', true, true))).toBe('overridden')
    expect(fileStatus(f('a', false, true))).toBe('override-only')
  })
})

describe('visibleRows', () => {
  const files = [
    f('assets/css/style.css'),
    f('assets/favicon.ico'),
    f('templates/base.html'),
    f('templates/partials/nav.html'),
  ]

  it('shows only top-level folders when nothing is expanded', () => {
    expect(visibleRows(files, new Set()).map((r) => r.path)).toEqual(['assets', 'templates'])
  })

  it('lists folders before files inside an expanded folder, with depth', () => {
    const rows = visibleRows(files, new Set(['templates', 'templates/partials']))
    expect(rows.map((r) => [r.kind, r.path, r.depth])).toEqual([
      ['folder', 'assets', 0],
      ['folder', 'templates', 0],
      ['folder', 'templates/partials', 1],
      ['file', 'templates/partials/nav.html', 2],
      ['file', 'templates/base.html', 1],
    ])
    expect(rows[4].file?.path).toBe('templates/base.html')
  })
})
