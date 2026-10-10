import { describe, it, expect, afterEach, vi } from 'vitest'
import { api, ApiError } from './api'

function respond(status: number, body: unknown) {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status })))
}

describe('api errors', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('carries the structured code and details of a rejection', async () => {
    respond(409, { error: 'changed outside', code: 'conflict', details: ['templates/a.html'] })
    const err = await api('/x').catch((e) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 409, message: 'changed outside', code: 'conflict', details: ['templates/a.html'] })
  })

  it('a plain error has neither', async () => {
    respond(404, { error: 'not found' })
    const err = await api('/x').catch((e) => e)
    expect(err).toMatchObject({ status: 404, message: 'not found', code: undefined, details: undefined })
  })
})
