import { describe, it, expect } from 'vitest'
import { ApiError } from '../api'
import { publishRejection } from './designPublish'

describe('publishRejection', () => {
  it('splits a 422 into one error per template problem', () => {
    const e = new ApiError(
      422,
      'templates/base.html:3: undefined value (rendering page `a; b` (anonymous)); templates/page.html: syntax error',
    )
    expect(publishRejection(e)).toEqual({
      kind: 'invalid',
      errors: [
        'templates/base.html:3: undefined value (rendering page `a; b` (anonymous))',
        'templates/page.html: syntax error',
      ],
    })
  })

  it('extracts the conflicting paths from a 409', () => {
    const message =
      'design/ changed outside the draft since it was started: assets/css/style.css, templates/base.html; discard the draft to adopt those changes, or publish with force=true to overwrite them'
    expect(publishRejection(new ApiError(409, message))).toEqual({
      kind: 'conflict',
      paths: ['assets/css/style.css', 'templates/base.html'],
      message,
    })
  })

  it('tells "nothing to publish" apart from a conflict', () => {
    const message = 'nothing to publish: the draft matches the live design'
    expect(publishRejection(new ApiError(409, message))).toEqual({ kind: 'nothing', message })
  })

  it('an unrecognised 409 is still a conflict, without paths', () => {
    expect(publishRejection(new ApiError(409, 'busy'))).toMatchObject({ kind: 'conflict', paths: [] })
  })

  it('rethrows other errors', () => {
    const e = new ApiError(503, 'storage unavailable')
    expect(() => publishRejection(e)).toThrow(e)
    expect(() => publishRejection(new TypeError('offline'))).toThrow('offline')
  })
})
