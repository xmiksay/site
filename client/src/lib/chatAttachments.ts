// Chat attachments (#132): files are uploaded first
// (`POST /api/assistant/sessions/{id}/attachments`), then the message names
// their paths in a trailing note — the model opens them on request with
// `file_read` / `design_read`, and the transcript turns the note back into
// links.
import { designFileUrl } from './designPaths'

/** Mirrors the server's per-file limit (`attachments::MAX_ATTACHMENT_SIZE`). */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024

/** A stored attachment, as the upload answers it. */
export interface ChatAttachment {
  path: string
  mimetype: string
  size: number
  /** `file`: a site file (`file_read`); `design`: a design-draft asset (`design_read`). */
  target: 'file' | 'design'
  file_id?: number
}

const HEADER = 'Attached files'
const NOTE = /(?:^|\n\n)Attached files \([^\n]*\):\n((?:- [^\n]+(?:\n|$))+)$/

/** The note appended to a message carrying `attachments`; '' for none. A
 *  Designer chat's attachments are all draft assets, any other chat's all
 *  site files, so one hint covers the list. */
export function attachmentNote(attachments: ChatAttachment[]): string {
  if (attachments.length === 0) return ''
  const how =
    attachments[0].target === 'design'
      ? 'open one with design_read {"path": …}'
      : 'look at one with file_read {"path": …, "include_content": true}'
  return `${HEADER} (${how}):\n${attachments.map((a) => `- ${a.path}`).join('\n')}`
}

/** `text` with a trailing attachment note split off into its paths. */
export function splitAttachments(text: string): { body: string; paths: string[] } {
  const match = NOTE.exec(text)
  if (!match) return { body: text, paths: [] }
  const paths = match[1]
    .split('\n')
    .filter((line) => line.startsWith('- '))
    .map((line) => line.slice(2).trim())
  return { body: text.slice(0, match.index), paths }
}

/** Where the admin opens an attachment: site files live under `uploads/`,
 *  anything else is a design-draft asset. */
export function attachmentUrl(path: string, thumbnail = false): string {
  if (!path.startsWith('uploads/')) return designFileUrl(path)
  const encoded = path.split('/').map(encodeURIComponent).join('/')
  return `/api/files/by-path/${encoded}${thumbnail ? '?thumbnail=true' : ''}`
}
