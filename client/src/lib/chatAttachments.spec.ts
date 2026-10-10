import { describe, it, expect } from 'vitest'
import { attachmentNote, attachmentUrl, DESIGNER_ACCEPT, splitAttachments, type ChatAttachment } from './chatAttachments'

const file = (path: string): ChatAttachment => ({ path, mimetype: 'image/png', size: 1, target: 'file' })
const asset = (path: string): ChatAttachment => ({ path, mimetype: 'image/png', size: 1, target: 'design' })

describe('chatAttachments', () => {
  it('attachmentNote names every path and the tool that opens it', () => {
    expect(attachmentNote([])).toBe('')
    const note = attachmentNote([file('uploads/chat/2026-10/a.png'), file('uploads/chat/2026-10/b.pdf')])
    expect(note).toContain('file_read')
    expect(note).toContain('"include_content": true')
    expect(note.split('\n').slice(1)).toEqual(['- uploads/chat/2026-10/a.png', '- uploads/chat/2026-10/b.pdf'])
    expect(attachmentNote([asset('assets/img/logo.png')])).toContain('design_read')
  })

  it('splitAttachments round-trips a note appended to a message', () => {
    const note = attachmentNote([file('uploads/chat/2026-10/a.png'), asset('assets/img/b.png')])
    expect(splitAttachments(`Look at this\n\n${note}`)).toEqual({
      body: 'Look at this',
      paths: ['uploads/chat/2026-10/a.png', 'assets/img/b.png'],
    })
    expect(splitAttachments(note)).toEqual({ body: '', paths: ['uploads/chat/2026-10/a.png', 'assets/img/b.png'] })
  })

  it('splitAttachments leaves ordinary text alone', () => {
    const text = 'Attached files (none):\nnot a list'
    expect(splitAttachments(text)).toEqual({ body: text, paths: [] })
    expect(splitAttachments('hello')).toEqual({ body: 'hello', paths: [] })
  })

  it('attachmentUrl links site files by path and draft assets raw', () => {
    expect(attachmentUrl('uploads/chat/2026-10/a b.png')).toBe('/api/files/by-path/uploads/chat/2026-10/a%20b.png')
    expect(attachmentUrl('uploads/chat/2026-10/a.png', true)).toBe(
      '/api/files/by-path/uploads/chat/2026-10/a.png?thumbnail=true',
    )
    expect(attachmentUrl('assets/img/logo.png', true)).toBe('/api/design/draft/assets/img/logo.png')
    expect(attachmentUrl('assets/fonts/inter.woff2')).toBe('/api/design/draft/assets/fonts/inter.woff2')
  })

  it('attachmentUrl links nothing outside the two attachment folders', () => {
    for (const path of [
      'uploads/chat/../secret.png',
      'uploads/chat//a.png',
      'assets/img/./a.png',
      'uploads/other/a.png',
      'templates/base.html',
      'assets/css/style.css',
      '/uploads/chat/a.png',
      'https://evil.example/a.png',
    ]) {
      expect(attachmentUrl(path), path).toBeNull()
    }
  })

  it('DESIGNER_ACCEPT lists exactly the image and font extensions the server takes', () => {
    expect(DESIGNER_ACCEPT.split(',')).toEqual([
      '.png', '.jpg', '.jpeg', '.gif', '.webp', '.avif', '.svg', '.ico',
      '.woff', '.woff2', '.ttf', '.otf', '.eot',
    ])
  })
})
