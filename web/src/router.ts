import { useEffect, useState } from 'react'

// Minimal pushState router; the app only has a handful of routes.

export function navigate(path: string, replace = false) {
  if (path === location.pathname) return
  if (replace) history.replaceState(null, '', path)
  else history.pushState(null, '', path)
  window.dispatchEvent(new PopStateEvent('popstate'))
}

export function usePath(): string {
  const [path, setPath] = useState(location.pathname)
  useEffect(() => {
    const onPop = () => setPath(location.pathname)
    window.addEventListener('popstate', onPop)
    return () => window.removeEventListener('popstate', onPop)
  }, [])
  return path
}

/** `/client/T123/...` → `T123`. */
export function teamFromPath(path: string): string | undefined {
  return parseClientPath(path).team
}

/** `/client/{team}[/{channel}[/thread/{ts}]]`, `/client/{team}/apps[/{app}]` or `/client/{team}/inspector` */
export function parseClientPath(path: string): {
  team?: string
  channel?: string
  thread?: string
  apps?: boolean
  app?: string
  inspector?: boolean
  search?: boolean
} {
  const apps = /^\/client\/([A-Z0-9]+)\/apps(?:\/([A-Z0-9]+))?/.exec(path)
  if (apps) return { team: apps[1], apps: true, app: apps[2] }
  const search = /^\/client\/([A-Z0-9]+)\/search/.exec(path)
  if (search) return { team: search[1], search: true }
  const inspector = /^\/client\/([A-Z0-9]+)\/inspector/.exec(path)
  if (inspector) return { team: inspector[1], inspector: true }
  const m = /^\/client\/([A-Z0-9]+)(?:\/([A-Z0-9]+)(?:\/thread\/([0-9.]+))?)?/.exec(path)
  return m ? { team: m[1], channel: m[2], thread: m[3] } : {}
}

export function clientPath(team: string, channel?: string, thread?: string): string {
  let p = `/client/${team}`
  if (channel) p += `/${channel}`
  if (channel && thread) p += `/thread/${thread}`
  return p
}

/** Query parameter from the current URL. */
export function queryParam(name: string): string | null {
  return new URLSearchParams(location.search).get(name)
}
