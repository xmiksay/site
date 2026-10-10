import { ApiError } from '../api'
import type { DesignHistoryEntry } from '../types'

/** How `POST /api/design/publish` ended, from the server's structured
 *  rejection (`code` + `details`, #119). */
export type PublishOutcome =
  | { kind: 'published'; entry: DesignHistoryEntry }
  /** 422 `invalid`: the compile check or strict smoke render failed. */
  | { kind: 'invalid'; errors: string[] }
  /** 409 `conflict`: `design/` changed outside the draft; `force` overwrites `paths`. */
  | { kind: 'conflict'; paths: string[]; message: string }
  /** 409 `nothing_to_publish`: the draft matches the live design. */
  | { kind: 'nothing'; message: string }

/** The outcome for a rejected publish; any other failure is rethrown. */
export function publishRejection(e: unknown): PublishOutcome {
  if (!(e instanceof ApiError)) throw e
  switch (e.code) {
    case 'invalid':
      return { kind: 'invalid', errors: e.details?.length ? e.details : [e.message] }
    case 'conflict':
      return { kind: 'conflict', paths: e.details ?? [], message: e.message }
    case 'nothing_to_publish':
      return { kind: 'nothing', message: e.message }
    default:
      throw e
  }
}
