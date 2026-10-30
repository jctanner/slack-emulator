// Search (search.messages / search.files) with Slack's modifiers.

import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { ApiError, type Message, type SlackFile } from './api'
import { useWs } from './context'
import { Mrkdwn, displayName } from './mrkdwn'
import { clientPath, navigate, queryParam } from './router'
import { useWorkspace } from './store'
import { formatSize } from './Composer'
import { relativeTime } from './MessageList'

type Match = Message & { channel: { id: string; name: string; is_im: boolean; is_private: boolean }; permalink: string }
type Results<T> = { total: number; matches: T[]; paging: { page: number; pages: number } }

export function SearchPage() {
  const { store, membership } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const [query, setQuery] = useState(() => queryParam('q') ?? '')
  const [submitted, setSubmitted] = useState(query)
  const [tab, setTab] = useState<'messages' | 'files'>('messages')
  const [page, setPage] = useState(1)
  const [messages, setMessages] = useState<Results<Match>>()
  const [files, setFiles] = useState<Results<SlackFile>>()
  const [error, setError] = useState<string>()
  const ctx = useMemo(() => ({ me: store.state.me, users, convs }), [store, users, convs])

  // Follow `?q=` when the sidebar search runs again.
  useEffect(() => {
    const onNav = () => {
      const q = queryParam('q') ?? ''
      setQuery(q)
      setSubmitted(q)
      setPage(1)
    }
    window.addEventListener('popstate', onNav)
    return () => window.removeEventListener('popstate', onNav)
  }, [])

  useEffect(() => {
    if (!submitted.trim()) return
    setError(undefined)
    store
      .call<{ messages: Results<Match>; files: Results<SlackFile> }>('search.all', { query: submitted, count: 20, page })
      .then(
        (r) => {
          setMessages(r.messages)
          setFiles(r.files)
        },
        (e) => setError(e instanceof ApiError ? e.code : 'network error'),
      )
  }, [submitted, page, store])

  const submit = (e: FormEvent) => {
    e.preventDefault()
    setPage(1)
    setSubmitted(query)
    history.replaceState(null, '', `${clientPath(membership.team.id)}/search?q=${encodeURIComponent(query)}`)
  }

  const current = tab === 'messages' ? messages : files
  return (
    <>
      <header className="main-head">
        <button className="back icon-button" onClick={() => navigate(clientPath(membership.team.id))} aria-label="Back">
          ‹
        </button>
        <h1>Search</h1>
      </header>
      <div className="page-body search-page">
        <form className="search-form" onSubmit={submit}>
          <input
            type="search"
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search messages and files"
            aria-label="Search"
          />
          <button type="submit" className="primary-small" disabled={!query.trim()}>
            Search
          </button>
        </form>
        <p className="muted small">
          Narrow it down with <code>in:#channel</code>, <code>in:@person</code>, <code>from:@person</code>, <code>before:2026-10-01</code>,{' '}
          <code>after:</code>, <code>on:today</code>, <code>has:link</code>, <code>has:pin</code>, <code>is:thread</code> and "quoted phrases".
        </p>
        {submitted && (
          <div className="tabs" role="tablist">
            {(['messages', 'files'] as const).map((t) => (
              <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? 'active' : ''} onClick={() => setTab(t)}>
                {t === 'messages' ? 'Messages' : 'Files'} {t === 'messages' ? (messages ? `(${messages.total})` : '') : files ? `(${files.total})` : ''}
              </button>
            ))}
          </div>
        )}
        {error && <p className="error">Search failed: {error}</p>}
        {tab === 'messages' && messages && (
          <ul className="search-results">
            {messages.matches.map((m) => (
              <li key={`${m.channel.id}-${m.ts}`}>
                <button className="search-hit" onClick={() => navigate(`${clientPath(membership.team.id, m.channel.id)}?ts=${m.ts}`)}>
                  <span className="search-hit-head">
                    <strong>{displayName(users[m.user ?? ''], m.username ?? 'unknown')}</strong>
                    <span className="muted small">
                      {m.channel.is_im ? 'Direct message' : `${m.channel.is_private ? '🔒︎' : '#'}${m.channel.name}`} · {relativeTime(m.ts)}
                    </span>
                  </span>
                  <Mrkdwn text={m.text || '(no text)'} ctx={ctx} />
                </button>
              </li>
            ))}
            {!messages.matches.length && <li className="muted">No messages match.</li>}
          </ul>
        )}
        {tab === 'files' && files && (
          <ul className="search-results">
            {files.matches.map((f) => (
              <li key={f.id}>
                <a className="search-hit" href={new URL(f.url_private_download!).pathname} download={f.name}>
                  <span className="search-hit-head">
                    <strong>{f.title || f.name}</strong>
                    <span className="muted small">
                      {f.pretty_type} · {formatSize(f.size ?? 0)}
                    </span>
                  </span>
                </a>
              </li>
            ))}
            {!files.matches.length && <li className="muted">No files match.</li>}
          </ul>
        )}
        {current && current.paging.pages > 1 && (
          <div className="search-pages">
            <button disabled={page <= 1} onClick={() => setPage((p) => p - 1)}>
              ‹ Newer
            </button>
            <span className="muted small">
              Page {current.paging.page} of {current.paging.pages}
            </span>
            <button disabled={page >= current.paging.pages} onClick={() => setPage((p) => p + 1)}>
              Older ›
            </button>
          </div>
        )}
      </div>
    </>
  )
}
