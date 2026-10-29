import { useEffect, useRef, useState } from 'react'
import type { WorkspaceStore } from './store'

export type ConnectionStatus = 'connecting' | 'connected' | 'reconnecting'

/**
 * Keeps a `/ws/client` connection open for the workspace. Every `hello`
 * (first connect and each reconnect) triggers a full reload so nothing
 * published while disconnected is missed.
 */
export function useRealtime(store: WorkspaceStore) {
  const [status, setStatus] = useState<ConnectionStatus>('connecting')
  const ws = useRef<WebSocket | undefined>(undefined)
  const lastTyping = useRef(new Map<string, number>())

  useEffect(() => {
    let closed = false
    let retries = 0
    let retryTimer: number | undefined
    let pingTimer: number | undefined

    const connect = () => {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws'
      const sock = new WebSocket(`${proto}://${location.host}/ws/client?team=${encodeURIComponent(store.state.teamId)}`)
      ws.current = sock
      sock.onmessage = (e) => {
        let ev: Record<string, any>
        try {
          ev = JSON.parse(e.data)
        } catch {
          return
        }
        if (ev.type === 'hello' || ev.type === 'emu_resync') {
          retries = 0
          store.load().then(() => setStatus('connected'), () => sock.close())
        } else if (ev.type !== 'pong') {
          store.applyEvent(ev)
        }
      }
      sock.onclose = () => {
        clearInterval(pingTimer)
        if (closed) return
        setStatus('reconnecting')
        retryTimer = window.setTimeout(connect, Math.min(30_000, 500 * 2 ** retries++))
      }
      // Keeps proxies from idling the connection out.
      pingTimer = window.setInterval(() => {
        if (sock.readyState === WebSocket.OPEN) sock.send(JSON.stringify({ type: 'ping' }))
      }, 25_000)
    }
    connect()
    return () => {
      closed = true
      clearTimeout(retryTimer)
      clearInterval(pingTimer)
      ws.current?.close()
    }
  }, [store])

  /** Tells others we're typing (throttled to once every 3s per conversation). */
  const sendTyping = (channel: string, thread_ts?: string) => {
    const key = thread_ts ? `${channel}/${thread_ts}` : channel
    const now = Date.now()
    if ((lastTyping.current.get(key) ?? 0) > now - 3000) return
    lastTyping.current.set(key, now)
    if (ws.current?.readyState === WebSocket.OPEN) {
      ws.current.send(JSON.stringify({ type: 'typing', channel, thread_ts }))
    }
  }

  /** Forget the throttle after sending, so typing again is announced immediately. */
  const typingDone = (channel: string, thread_ts?: string) => {
    lastTyping.current.delete(thread_ts ? `${channel}/${thread_ts}` : channel)
  }

  return { status, sendTyping, typingDone }
}
