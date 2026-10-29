// The Inspector: a live view of this workspace's Web API calls, deliveries
// to apps and events, for owners and admins debugging bots.

import { useEffect, useMemo, useRef, useState } from 'react'
import { ApiError } from './api'
import { useWs } from './context'
import { displayName, plainText } from './mrkdwn'
import { clientPath, navigate } from './router'
import { convName, useWorkspace } from './store'

type Tab = 'api' | 'deliveries' | 'events'

type ApiCall = {
  seq: number
  time: number
  method: string
  via: 'token' | 'session' | 'harness' | 'anonymous'
  user_id?: string | null
  app_id?: string | null
  token_type?: string | null
  token?: string | null
  args: Record<string, unknown>
  status: number
  ok: boolean
  error?: string | null
  response: unknown
  latency_ms: number
}

type DeliveryRow = {
  seq: number
  time: number
  app_id: string
  label: string
  transport: string
  attempt: number
  ok: boolean
  error?: string | null
  latency_ms: number
  payload: unknown
  response?: unknown
}

type EventRow = { seq: number; ui_only: boolean; event: Record<string, any> }

const MAX_ROWS = 500

/** Polls an `emu.inspector.*` method for entries after the last one seen, newest first. */
function useStream<T extends { seq: number }>(method: string, params: Record<string, unknown>, paused: boolean) {
  const { store } = useWs()
  const [rows, setRows] = useState<T[]>([])
  const [error, setError] = useState<string>()
  const [extra, setExtra] = useState<Record<string, any>>({})
  const after = useRef(0)
  const busy = useRef(false)
  const key = JSON.stringify(params)

  useEffect(() => {
    after.current = 0
    setRows([])
  }, [method, key])

  useEffect(() => {
    if (paused) return
    let stopped = false
    const load = async () => {
      if (busy.current) return
      busy.current = true
      try {
        const r = await store.call<{ entries: T[] } & Record<string, any>>(method, {
          ...params,
          after: after.current || undefined,
        })
        if (stopped) return
        setError(undefined)
        setExtra(r)
        if (r.entries.length) {
          after.current = r.entries[r.entries.length - 1].seq
          setRows((prev) => [...r.entries.slice().reverse(), ...prev].slice(0, MAX_ROWS))
        }
      } catch (e) {
        if (!stopped) setError(e instanceof ApiError ? e.code : 'network error')
      } finally {
        busy.current = false
      }
    }
    load()
    const t = setInterval(load, 1500)
    return () => {
      stopped = true
      clearInterval(t)
    }
  }, [method, key, paused, store])

  return { rows, error, extra, clear: () => setRows([]) }
}

function timeOfMs(ms: number) {
  return new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

export function InspectorPage() {
  const { membership } = useWs()
  const [tab, setTab] = useState<Tab>('api')
  const [filter, setFilter] = useState('')
  const [paused, setPaused] = useState(false)
  const [withUi, setWithUi] = useState(false)

  return (
    <>
      <header className="main-head">
        <button className="back icon-button" onClick={() => navigate(clientPath(membership.team.id))} aria-label="Back">
          ‹
        </button>
        <h1>Inspector</h1>
      </header>
      <div className="page-body inspector">
        <p className="muted small page-lead">
          Live Web API calls, deliveries to apps and events in {membership.team.name}. Scripts get the same data, across
          every workspace, from the test harness at <code>/_emu</code>.
        </p>
        <div className="tabs" role="tablist">
          {(
            [
              ['api', 'API calls'],
              ['deliveries', 'Deliveries'],
              ['events', 'Events'],
            ] as [Tab, string][]
          ).map(([t, label]) => (
            <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? 'active' : ''} onClick={() => setTab(t)}>
              {label}
            </button>
          ))}
        </div>
        <div className="inspector-bar">
          <input
            className="inspector-filter"
            type="search"
            placeholder="Filter (method, channel, text…)"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter"
          />
          {tab === 'api' && (
            <label className="inspector-toggle">
              <input type="checkbox" checked={withUi} onChange={(e) => setWithUi(e.target.checked)} /> Web UI calls
            </label>
          )}
          <button className="inspector-button" onClick={() => setPaused((p) => !p)} aria-pressed={paused}>
            {paused ? '▶︎ Resume' : '❚❚ Pause'}
          </button>
        </div>
        {tab === 'api' && <ApiCalls filter={filter} paused={paused} withUi={withUi} />}
        {tab === 'deliveries' && <Deliveries filter={filter} paused={paused} />}
        {tab === 'events' && <Events filter={filter} paused={paused} />}
      </div>
    </>
  )
}

function useFiltered<T>(rows: T[], filter: string) {
  return useMemo(() => {
    const q = filter.trim().toLowerCase()
    return q ? rows.filter((r) => JSON.stringify(r).toLowerCase().includes(q)) : rows
  }, [rows, filter])
}

function StreamState({ error, count, empty, children }: { error?: string; count: number; empty: string; children: React.ReactNode }) {
  if (error === 'not_allowed') return <p className="empty-state">The Inspector is for workspace owners and admins.</p>
  if (error) return <p className="error">Couldn’t load: {error}</p>
  if (count === 0) return <p className="empty-state">{empty}</p>
  return <>{children}</>
}

function Json({ label, value }: { label: string; value: unknown }) {
  return (
    <>
      <div className="inspector-json-label">{label}</div>
      <pre className="event-payload">{JSON.stringify(value, null, 2)}</pre>
    </>
  )
}

function ApiCalls({ filter, paused, withUi }: { filter: string; paused: boolean; withUi: boolean }) {
  const { store } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const params = useMemo(() => ({ include_session: withUi }), [withUi])
  const { rows, error } = useStream<ApiCall>('emu.inspector.apiLog', params, paused)
  const shown = useFiltered(rows, filter)
  const [open, setOpen] = useState<number | null>(null)

  const who = (c: ApiCall) => {
    const name = c.user_id ? displayName(users[c.user_id], c.user_id) : undefined
    if (c.via === 'token') return `${c.token ?? c.token_type ?? 'token'}${name ? ` · ${name}` : ''}`
    if (c.via === 'harness') return `harness as ${name ?? '?'}`
    if (c.via === 'session') return `web UI · ${name ?? '?'}`
    return 'no token'
  }

  return (
    <StreamState error={error} count={shown.length} empty="No API calls yet. Calls from bots (and the harness) appear here as they happen.">
      <ul className="event-log">
        {shown.map((c) => (
          <li key={c.seq} className={c.ok ? '' : 'failed'}>
            <button className="event-row inspector-row" onClick={() => setOpen(open === c.seq ? null : c.seq)} aria-expanded={open === c.seq}>
              <span className={`dot ${c.ok ? 'ok' : 'bad'}`} aria-hidden />
              <span className="event-label">{c.method}</span>
              <span className="muted small inspector-who">{who(c)}</span>
              <span className="muted small event-result">
                {c.ok ? `${c.latency_ms} ms` : `${c.error}${c.status !== 200 ? ` (${c.status})` : ''}`}
              </span>
              <time className="muted small">{timeOfMs(c.time)}</time>
            </button>
            {open === c.seq && (
              <div className="inspector-detail">
                <Json label="Arguments" value={c.args} />
                <Json label="Response" value={c.response} />
              </div>
            )}
          </li>
        ))}
      </ul>
    </StreamState>
  )
}

function Deliveries({ filter, paused }: { filter: string; paused: boolean }) {
  const params = useMemo(() => ({}), [])
  const { rows, error } = useStream<DeliveryRow>('emu.inspector.deliveries', params, paused)
  const shown = useFiltered(rows, filter)
  const [open, setOpen] = useState<number | null>(null)
  const [apps, setApps] = useState<Record<string, string>>({})
  const { store } = useWs()
  useEffect(() => {
    store
      .call<{ apps: { id: string; name: string }[] }>('emu.apps.list')
      .then((r) => setApps(Object.fromEntries(r.apps.map((a) => [a.id, a.name]))), () => {})
  }, [store, rows.length > 0])

  return (
    <StreamState error={error} count={shown.length} empty="Nothing delivered to apps yet.">
      <ul className="event-log">
        {shown.map((d) => (
          <li key={d.seq} className={d.ok ? '' : 'failed'}>
            <button className="event-row inspector-row" onClick={() => setOpen(open === d.seq ? null : d.seq)} aria-expanded={open === d.seq}>
              <span className={`dot ${d.ok ? 'ok' : 'bad'}`} aria-hidden />
              <span className="event-label">{d.label}</span>
              <span className="muted small inspector-who">
                {apps[d.app_id] ?? d.app_id} · {d.transport}
                {d.attempt > 0 && ` · retry ${d.attempt}`}
              </span>
              <span className="muted small event-result">{d.ok ? `${d.latency_ms} ms` : d.error}</span>
              <time className="muted small">{timeOfMs(d.time * 1000)}</time>
            </button>
            {open === d.seq && (
              <div className="inspector-detail">
                <Json label={d.transport === 'inbound' ? 'Sent by the app' : 'Payload'} value={d.payload} />
                {d.response !== undefined && d.response !== null && <Json label="App’s response" value={d.response} />}
              </div>
            )}
          </li>
        ))}
      </ul>
    </StreamState>
  )
}

function Events({ filter, paused }: { filter: string; paused: boolean }) {
  const { store } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const params = useMemo(() => ({}), [])
  const { rows, error, extra } = useStream<EventRow>('emu.inspector.events', params, paused)
  const shown = useFiltered(rows, filter)
  const [open, setOpen] = useState<number | null>(null)
  const limits = extra.rate_limits as { enabled: boolean } | undefined

  const summary = (e: Record<string, any>) => {
    const parts: string[] = []
    const channel = e.channel?.id ?? e.channel ?? e.item?.channel
    if (typeof channel === 'string') {
      const c = convs[channel]
      parts.push(c ? (c.is_im || c.is_mpim ? convName(c, { users, me: store.state.me }) : `#${c.name}`) : channel)
    }
    const user = e.user?.id ?? e.user ?? e.message?.user
    if (typeof user === 'string') parts.push(displayName(users[user], user))
    const text = e.text ?? e.message?.text
    if (typeof text === 'string' && text) {
      const t = plainText(text, { users, convs })
      parts.push(`“${t.length > 60 ? `${t.slice(0, 60)}…` : t}”`)
    }
    return parts.join(' · ')
  }

  return (
    <>
      {limits && (
        <p className="muted small">
          Rate limits are <strong>{limits.enabled ? 'on' : 'off'}</strong> (set with <code>/_emu/rate-limits</code>).
        </p>
      )}
      <StreamState error={error} count={shown.length} empty="No events yet.">
        <ul className="event-log">
          {shown.map((r) => (
            <li key={r.seq}>
              <button className="event-row inspector-row" onClick={() => setOpen(open === r.seq ? null : r.seq)} aria-expanded={open === r.seq}>
                <span className="dot ok" aria-hidden />
                <span className="event-label">
                  {r.event.type}
                  {r.event.subtype ? `/${r.event.subtype}` : ''}
                </span>
                <span className="muted small inspector-who">{summary(r.event)}</span>
                <span className="muted small event-result">{r.ui_only ? 'web UI only' : ''}</span>
                <time className="muted small">#{r.seq}</time>
              </button>
              {open === r.seq && (
                <div className="inspector-detail">
                  <Json label="Event" value={r.event} />
                </div>
              )}
            </li>
          ))}
        </ul>
      </StreamState>
    </>
  )
}
