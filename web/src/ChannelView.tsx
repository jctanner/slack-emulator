import { useEffect, useMemo, useState } from 'react'
import type { Conversation } from './api'
import { AboutTab, AppTabs, HomeTab, useAppDm, useTabOpened, type AppTab } from './AppHome'
import { Composer, TypingIndicator } from './Composer'
import { useWs } from './context'
import { MessageList } from './MessageList'
import { Mrkdwn } from './mrkdwn'
import { clientPath, navigate, queryParam } from './router'
import { convName, mergeMessages, useWorkspace } from './store'

export function ChannelView({ channel }: { channel: string }) {
  const ws = useWs()
  const { store, membership } = ws
  const conv = useWorkspace(store, (s) => s.convs[channel])
  const list = useWorkspace(store, (s) => s.channels[channel])
  const ephemerals = useWorkspace(store, (s) => s.ephemerals[channel])
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const [error, setError] = useState<string>()
  const [highlight] = useState(() => queryParam('ts'))
  const msgs = useMemo(
    () => mergeMessages(list?.msgs ?? [], (ephemerals ?? []).filter((m) => !m.thread_ts || m.thread_ts === m.ts)),
    [list?.msgs, ephemerals],
  )

  // A DM with an app's bot gets Home / Messages / About tabs.
  const app = useAppDm(channel, !!conv?.is_im && !!users[conv.user ?? '']?.is_bot)
  const [tab, setTab] = useState<AppTab | null>(null)
  useEffect(() => {
    if (app) setTab((t) => t ?? (app.home_tab_enabled ? 'home' : app.messages_tab_enabled ? 'messages' : 'about'))
  }, [app])
  useTabOpened(channel, app, tab)

  // Fresh details (member count; or the whole conversation when previewing one we're not in).
  useEffect(() => {
    store
      .call<{ channel: Conversation }>('conversations.info', { channel, include_num_members: true })
      .then((r) => store.upsertConv(r.channel), (e) => setError(e.code ?? 'error'))
  }, [channel, store])

  useEffect(() => {
    if (!list && conv) store.loadHistory(channel).catch((e) => setError(e.code ?? 'error'))
  }, [channel, list, conv, store])

  // Read tracking: mark read while this channel is visible and focused.
  useEffect(() => {
    const update = () => {
      store.viewing = { channel, focused: document.hasFocus() && document.visibilityState === 'visible' }
      const last = store.state.channels[channel]?.msgs.filter((m) => !m.pending).at(-1)
      if (store.viewing.focused && last && conv?.is_member) store.markRead(channel, last.ts)
    }
    update()
    window.addEventListener('focus', update)
    window.addEventListener('blur', update)
    document.addEventListener('visibilitychange', update)
    return () => {
      store.viewing = undefined
      window.removeEventListener('focus', update)
      window.removeEventListener('blur', update)
      document.removeEventListener('visibilitychange', update)
    }
  }, [channel, store, conv?.is_member, list?.msgs.length])

  if (error) {
    return (
      <div className="pane-loading">
        {error === 'channel_not_found' ? 'That conversation doesn’t exist or you don’t have access.' : `Couldn’t load: ${error}`}
      </div>
    )
  }
  if (!conv) return <div className="pane-loading">Loading…</div>

  const name = convName(conv, { users, me: store.state.me })
  const isDm = conv.is_im || conv.is_mpim
  const ctx = { me: store.state.me, users, convs, onChannel: (id: string) => ws.open(id) }
  const details = (tab?: 'about' | 'members' | 'settings') => ws.showDialog({ kind: 'channel-details', channel, tab })

  const join = async () => {
    try {
      const r = await store.call<{ channel: Conversation }>('conversations.join', { channel })
      store.upsertConv({ ...r.channel, is_member: true })
    } catch (e: any) {
      alert(`Couldn’t join: ${e.code ?? e}`)
    }
  }

  const intro = (
    <div className="channel-intro">
      {isDm ? (
        <>
          <h2>{name}</h2>
          <p className="muted">
            This is the very beginning of your direct message history with <strong>{name}</strong>.
          </p>
        </>
      ) : (
        <>
          <h2>
            {conv.is_private ? '🔒︎' : '#'} {conv.name}
          </h2>
          <p className="muted">
            {conv.creator && users[conv.creator]
              ? `@${users[conv.creator].name} created this channel. `
              : ''}
            This is the very beginning of the <strong>#{conv.name}</strong> channel.
            {conv.purpose?.value && <> {conv.purpose.value}</>}
          </p>
        </>
      )}
    </div>
  )

  return (
    <>
      <header className="main-head channel-head">
        <button className="back icon-button" onClick={() => navigate(clientPath(membership.team.id))} aria-label="Back">
          ‹
        </button>
        <button className="channel-title" onClick={() => details()}>
          <span className="channel-title-name">
            {isDm ? '' : conv.is_private ? '🔒︎ ' : '# '}
            {name}
          </span>
          <span className="caret" aria-hidden>
            ▾
          </span>
        </button>
        {conv.topic?.value && (
          <span className="channel-topic" title={conv.topic.value}>
            <Mrkdwn text={conv.topic.value} ctx={ctx} />
          </span>
        )}
        {!conv.is_im && (
          <button className="member-count" onClick={() => details('members')} title="View members">
            👤︎ {conv.num_members ?? ''}
          </button>
        )}
      </header>
      {conv.is_archived && <div className="archived-banner">You’re viewing an archived channel.</div>}
      {app && <AppTabs app={app} tab={tab} onTab={setTab} />}
      {app && tab === 'home' ? (
        <HomeTab app={app} />
      ) : app && tab === 'about' ? (
        <AboutTab app={app} />
      ) : (
        <>
          <MessageList
            channel={channel}
            msgs={msgs}
            mode="channel"
            hasMore={list?.hasMore}
            loadingOlder={list?.loading}
            onLoadOlder={() => store.loadHistory(channel, true)}
            highlight={highlight}
            header={list && !list.loading ? intro : undefined}
          />
          {conv.is_archived ? null : app?.messages_tab_read_only ? (
            <div className="join-bar">
              <p className="muted">Sending messages to this app has been turned off.</p>
            </div>
          ) : conv.is_member ? (
            <>
              <Composer channel={channel} placeholder={`Message ${isDm ? name : `#${conv.name}`}`} />
              <TypingIndicator keyName={channel} />
            </>
          ) : (
            <div className="join-bar">
              <p>
                You’re viewing <strong>#{conv.name}</strong>
              </p>
              <button className="primary-small" onClick={join}>
                Join channel
              </button>
            </div>
          )}
        </>
      )}
    </>
  )
}
