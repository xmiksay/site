import { ApiError } from '../api'
import type { DesignHistoryEntry } from '../types'

/** How `POST /api/design/publish` ended, with the server's rejections parsed
 *  into something the toolbar can show (#119). */
export type PublishOutcome =
  | { kind: 'published'; entry: DesignHistoryEntry }
  /** 422: the compile check or strict smoke render failed. */
  | { kind: 'invalid'; errors: string[] }
  /** 409: `design/` changed outside the draft; `force` overwrites `paths`. */
  | { kind: 'conflict'; paths: string[]; message: string }
  /** 409: the draft matches the live design. */
  | { kind: 'nothing'; message: string }

// The server joins one message per problem with "; " (`DesignError::Invalid`);
// a message may itself contain "; ", so only split where the next problem
// starts with a bundle path.
const ERROR_SEPARATOR = /; (?=(?:templates|assets|mdcast)\/)/

// Mirrors `DesignError::Conflict`'s text: "...since it was started: a, b; discard...".
const CONFLICT_PATHS = /since it was started: (.*?); discard/

/** The outcome for a rejected publish; anything but a 409/422 is rethrown. */
export function publishRejection(e: unknown): PublishOutcome {
  if (!(e instanceof ApiError)) throw e
  if (e.status === 422) return { kind: 'invalid', errors: e.message.split(ERROR_SEPARATOR) }
  if (e.status !== 409) throw e
  if (e.message.startsWith('nothing to publish')) return { kind: 'nothing', message: e.message }
  const paths = CONFLICT_PATHS.exec(e.message)?.[1].split(', ') ?? []
  return { kind: 'conflict', paths, message: e.message }
}
