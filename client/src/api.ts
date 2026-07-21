export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function parseError(resp: Response): Promise<string> {
  try {
    const body = await resp.json()
    if (body && typeof body.error === 'string') return body.error
  } catch {
    // ignore
  }
  return `${resp.status} ${resp.statusText}`
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body && !(init.body instanceof FormData) && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }
  const resp = await fetch(path, { ...init, credentials: 'include', headers })
  if (!resp.ok) {
    throw new ApiError(resp.status, await parseError(resp))
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
    throw new ApiError(resp.status, await parseError(resp))
  }
  const disposition = resp.headers.get('Content-Disposition')
  const match = disposition ? /filename="?([^"]+)"?/.exec(disposition) : null
  const filename = match?.[1] ?? 'download'
  return { blob: await resp.blob(), filename }
}
