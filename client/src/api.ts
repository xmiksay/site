export class ApiError extends Error {
  status: number
  /** Machine-readable kind of a rejection the client acts on (e.g. `conflict`). */
  code?: string
  /** The rejection's items (failing templates, conflicting paths). */
  details?: string[]
  constructor(status: number, message: string, code?: string, details?: string[]) {
    super(message)
    this.status = status
    this.code = code
    this.details = details
  }
}

async function parseError(resp: Response): Promise<ApiError> {
  try {
    const body = await resp.json()
    if (body && typeof body.error === 'string') {
      const code = typeof body.code === 'string' ? body.code : undefined
      const details = Array.isArray(body.details) ? body.details.map(String) : undefined
      return new ApiError(resp.status, body.error, code, details)
    }
  } catch {
    // ignore
  }
  return new ApiError(resp.status, `${resp.status} ${resp.statusText}`)
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body && !(init.body instanceof FormData) && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }
  const resp = await fetch(path, { ...init, credentials: 'include', headers })
  if (!resp.ok) {
    throw await parseError(resp)
  }
  if (resp.status === 204) {
    return undefined as T
  }
  return (await resp.json()) as T
}

export function apiVoid(path: string, init: RequestInit = {}): Promise<void> {
  return api<void>(path, init)
}

export async function apiBlob(
  path: string,
  init: RequestInit = {},
): Promise<{ blob: Blob; filename: string }> {
  const resp = await fetch(path, { ...init, credentials: 'include' })
  if (!resp.ok) {
    throw await parseError(resp)
  }
  const disposition = resp.headers.get('Content-Disposition')
  const match = disposition ? /filename="?([^"]+)"?/.exec(disposition) : null
  const filename = match?.[1] ?? 'download'
  return { blob: await resp.blob(), filename }
}
