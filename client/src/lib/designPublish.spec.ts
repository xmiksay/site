import { describe, it, expect } from 'vitest'
import { ApiError } from '../api'
import { publishRejection } from './designPublish'

describe('publishRejection', () => {
  it('a 422 `invalid` lists the server-provided errors', () => {
    const errors = ['templates/base.html:3: undefined value (rendering 404)', 'templates/page.html: syntax error']
    const e = new ApiError(422, errors.join('; '), 'invalid', errors)
    expect(publishRejection(e)).toEqual({ kind: 'invalid', errors })
  })

  it('an `invalid` without details falls back to the message', () => {
    expect(publishRejection(new ApiError(422, 'broken', 'invalid'))).toEqual({
      kind: 'invalid',
      errors: ['broken'],
    })
  })

  it('a 409 `conflict` carries the paths', () => {
    const e = new ApiError(409, 'changed outside', 'conflict', ['assets/css/style.css', 'templates/base.html'])
    expect(publishRejection(e)).toEqual({
      kind: 'conflict',
      paths: ['assets/css/style.css', 'templates/base.html'],
      message: 'changed outside',
    })
  })

  it('tells `nothing_to_publish` apart from a conflict', () => {
    const message = 'nothing to publish: the draft matches the live design'
    expect(publishRejection(new ApiError(409, message, 'nothing_to_publish'))).toEqual({
      kind: 'nothing',
      message,
    })
  })

  it('rethrows uncoded and other errors', () => {
    const uncoded = new ApiError(409, 'busy')
    expect(() => publishRejection(uncoded)).toThrow(uncoded)
    const e = new ApiError(503, 'storage unavailable')
    expect(() => publishRejection(e)).toThrow(e)
    expect(() => publishRejection(new TypeError('offline'))).toThrow('offline')
  })
})
