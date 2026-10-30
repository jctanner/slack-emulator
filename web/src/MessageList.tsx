import { Fragment, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import type { Message, SlackFile } from './api'
import { useWs } from './context'
import { MessageBlocks, type Container } from './BlockKit'
import { formatSize } from './Composer'
import { ShortcutMenu } from './Shortcuts'
import { EmojiPicker } from './EmojiPicker'
import { emojiChar } from './emoji'
import { Mrkdwn, decodeForEdit, displayName, encodeOutgoing, type MrkdwnContext } from './mrkdwn'
import { useWorkspace } from './store'
import { Avatar } from './Avatar'

/** System subtypes rendered as a muted one-liner. */
const SYSTEM = new Set([
  'channel_join',
  'channel_leave',
  'channel_topic',
  'channel_purpose',
  'channel_name',
  'channel_archive',
  'channel_unarchive',
  'pinned_item',
])

const GROUP_WINDOW_SECS = 5 * 60

export function tsDate(ts: string) {
  return new Date(parseFloat(ts) * 1000)
}

export function timeOf(ts: string) {
  return tsDate(ts).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
}

function dayLabel(d: Date) {
  const today = new Date()
  const yesterday = new Date(Date.now() - 86_400_000)
  if (d.toDateString() === today.toDateString()) return 'Today'
  if (d.toDateString() === yesterday.toDateString()) return 'Yesterday'
  return d.toLocaleDateString([], {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
    year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric',
  })
}

export function relativeTime(ts: string) {
  const secs = Date.now() / 1000 - parseFloat(ts)
  if (secs < 60) return 'just now'
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`
  if (secs < 86400) return `${Math.floor(secs / 3600)} hr ago`
  return dayLabel(tsDate(ts))
}

export function MessageList({
  channel,
  msgs,
  mode,
  hasMore = false,
  loadingOlder = false,
  onLoadOlder,
  highlight,
  header,
}: {
  channel: string
  msgs: Message[]
  mode: 'channel' | 'thread'
  hasMore?: boolean
  loadingOlder?: boolean
  onLoadOlder?: () => void
  highlight?: string | null
  header?: React.ReactNode
}) {
  const { store } = useWs()
  const scroller = useRef<HTMLDivElement>(null)
  const stick = useRef(true)
  const prev = useRef<{ first?: string; height: number; top: number }>({ height: 0, top: 0 })
  const [picker, setPicker] = useState<{ ts: string; rect: DOMRect } | null>(null)
  const [editing, setEditing] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)

  // Keep the view anchored: stick to the bottom for new messages, and hold
  // position when older messages are prepended.
  useLayoutEffect(() => {
    const el = scroller.current
    if (!el) return
    const first = msgs[0]?.ts
    if (prev.current.first && first && first < prev.current.first) {
      el.scrollTop = el.scrollHeight - prev.current.height + prev.current.top
    } else if (stick.current) {
      el.scrollTop = el.scrollHeight
    }
    prev.current = { first, height: el.scrollHeight, top: el.scrollTop }
  }, [msgs])

  // Jump to a linked message once it's loaded.
  useLayoutEffect(() => {
    if (!highlight) return
    const el = scroller.current?.querySelector(`[data-ts="${CSS.escape(highlight)}"]`)
    if (el) {
      stick.current = false
      el.scrollIntoView({ block: 'center' })
    }
  }, [highlight, msgs.length > 0])

  const onScroll = () => {
    const el = scroller.current!
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80
    prev.current = { ...prev.current, height: el.scrollHeight, top: el.scrollTop }
    if (el.scrollTop < 300 && hasMore && !loadingOlder) onLoadOlder?.()
  }

  return (
    <div className={`message-scroller ${mode}`} ref={scroller} onScroll={onScroll}>
      {hasMore ? <div className="history-loading">{loadingOlder ? 'Loading…' : ''}</div> : header}
      <div className="messages">
        {msgs.map((m, i) => {
          const prevMsg = msgs[i - 1]
          const newDay = !prevMsg || tsDate(prevMsg.ts).toDateString() !== tsDate(m.ts).toDateString()
          const compact =
            !newDay &&
            !!prevMsg &&
            !(mode === 'thread' && i === 1) &&
            prevMsg.user === m.user &&
            prevMsg.username === m.username &&
            !SYSTEM.has(m.subtype ?? '') &&
            !SYSTEM.has(prevMsg.subtype ?? '') &&
            m.subtype !== 'thread_broadcast' &&
            !m.is_ephemeral &&
            !prevMsg.is_ephemeral &&
            !(mode === 'channel' && (prevMsg.reply_count ?? 0) > 0) &&
            parseFloat(m.ts) - parseFloat(prevMsg.ts) < GROUP_WINDOW_SECS
          return (
            <Fragment key={m.client_msg_id && m.pending ? m.client_msg_id : m.ts}>
              {newDay && !m.pending && (
                <div className="day-divider">
                  <span>{dayLabel(tsDate(m.ts))}</span>
                </div>
              )}
              <MessageRow
                channel={channel}
                msg={m}
                mode={mode}
                compact={compact}
                highlighted={m.ts === highlight}
                selected={selected === m.ts}
                editing={editing === m.ts}
                onSelect={() => setSelected((s) => (s === m.ts ? null : m.ts))}
                onEdit={(on) => setEditing(on ? m.ts : null)}
                onReact={(rect) => setPicker({ ts: m.ts, rect })}
              />
              {mode === 'thread' && i === 0 && msgs.length > 1 && (
                <div className="thread-divider">
                  <span>
                    {msgs.length - 1} {msgs.length === 2 ? 'reply' : 'replies'}
                  </span>
                </div>
              )}
            </Fragment>
          )
        })}
      </div>
      {picker && (
        <EmojiPicker
          anchor={picker.rect}
          onClose={() => setPicker(null)}
          onPick={(name) => {
            setPicker(null)
            toggleReaction(channel, picker.ts, name, msgs.find((m) => m.ts === picker.ts))
          }}
        />
      )}
    </div>
  )

  function toggleReaction(ch: string, ts: string, name: string, m?: Message) {
    const mine = m?.reactions?.find((r) => r.name === name)?.users.includes(store.state.me)
    store.call(mine ? 'reactions.remove' : 'reactions.add', { channel: ch, timestamp: ts, name }).catch(() => {})
  }
}

function MessageRow({
  channel,
  msg,
  mode,
  compact,
  highlighted,
  selected,
  editing,
  onSelect,
  onEdit,
  onReact,
}: {
  channel: string
  msg: Message
  mode: 'channel' | 'thread'
  compact: boolean
  highlighted: boolean
  selected: boolean
  editing: boolean
  onSelect: () => void
  onEdit: (on: boolean) => void
  onReact: (rect: DOMRect) => void
}) {
  const ws = useWs()
  const users = useWorkspace(ws.store, (s) => s.users)
  const convs = useWorkspace(ws.store, (s) => s.convs)
  const me = ws.store.state.me
  const author = users[msg.user ?? '']
  const meUser = users[me]
  const ctx: MrkdwnContext = useMemo(() => ({ me, users, convs, onChannel: (id) => ws.open(id) }), [me, users, convs, ws])

  const isBot = !!msg.bot_id || !!author?.is_bot
  const name = msg.username || displayName(author, msg.user === 'USLACKBOT' ? 'Slackbot' : msg.bot_id ? 'bot' : 'unknown')
  const own = msg.user === me
  const system = SYSTEM.has(msg.subtype ?? '')
  const tombstone = msg.subtype === 'tombstone'
  const ephemeral = !!msg.is_ephemeral
  const hasBlocks = !!msg.blocks?.length
  const container = useMemo<Container>(
    () => ({ type: 'message', channel_id: channel, message_ts: msg.ts, is_ephemeral: ephemeral }),
    [channel, msg.ts, ephemeral],
  )
  const canEdit = own && !system && !tombstone && !msg.pending && !msg.failed
  const canDelete = (own || meUser?.is_admin || meUser?.is_owner) && !system && !tombstone && !msg.pending && !msg.failed

  const del = async () => {
    if (!confirm('Delete this message? This can’t be undone.')) return
    await ws.store.call('chat.delete', { channel, ts: msg.ts }).catch((e) => alert(`Couldn’t delete: ${e.code ?? e}`))
  }

  const copyLink = async () => {
    try {
      const r = await ws.store.call<{ permalink: string }>('chat.getPermalink', { channel, message_ts: msg.ts })
      await navigator.clipboard.writeText(r.permalink)
    } catch {
      // Clipboard may be unavailable (insecure context); ignore.
    }
  }

  const classes = [
    'message',
    compact ? 'compact' : '',
    system || tombstone ? 'system' : '',
    msg.pending ? 'pending' : '',
    msg.failed ? 'failed' : '',
    highlighted ? 'highlighted' : '',
    msg.pinned_to?.length ? 'pinned' : '',
    selected ? 'selected' : '',
    ephemeral ? 'ephemeral' : '',
  ].join(' ')

  return (
    <div className={classes} data-ts={msg.ts} onClick={(e) => e.target === e.currentTarget && onSelect()}>
      <div className="message-gutter">
        {compact ? (
          <span className="gutter-time">{timeOf(msg.ts)}</span>
        ) : (
          <Avatar user={author} id={msg.user ?? msg.bot_id ?? msg.ts} icons={msg.icons} />
        )}
      </div>
      <div className="message-content">
        {!!msg.pinned_to?.length && (
          <div className="pinned-note">
            📌︎ Pinned{msg.pinned_by ? ` by ${displayName(users[msg.pinned_by], msg.pinned_by)}` : ''}
          </div>
        )}
        {ephemeral && (
          <div className="ephemeral-note">
            <span aria-hidden>👁︎</span> Only visible to you ·{' '}
            <button className="link" onClick={() => ws.store.dismissEphemeral(channel, msg.ts)}>
              Dismiss
            </button>
          </div>
        )}
        {!compact && (
          <div className="message-head">
            <span className="author">{name}</span>
            {isBot && <span className="app-badge">APP</span>}
            {!msg.pending && (
              <time dateTime={tsDate(msg.ts).toISOString()} title={tsDate(msg.ts).toLocaleString()}>
                {timeOf(msg.ts)}
              </time>
            )}
          </div>
        )}
        {msg.subtype === 'thread_broadcast' && mode === 'channel' && (
          <button className="broadcast-label" onClick={() => ws.open(channel, msg.thread_ts)}>
            replied to a thread
          </button>
        )}
        {editing ? (
          <EditBox channel={channel} msg={msg} onDone={() => onEdit(false)} />
        ) : (
          <div
            className="message-body"
            onClick={(e) => !(e.target as HTMLElement).closest('button, a, select, input, textarea, label') && onSelect()}
          >
            {tombstone ? (
              <span className="muted">This message was deleted.</span>
            ) : msg.subtype === 'me_message' ? (
              <em>
                <Mrkdwn text={msg.text} ctx={ctx} />
              </em>
            ) : hasBlocks ? (
              // Blocks replace the text, which is only a fallback.
              <>
                <MessageBlocks blocks={msg.blocks} attachments={msg.attachments} container={container} ctx={ctx} />
                {msg.edited && <span className="edited">(edited)</span>}
              </>
            ) : (
              <>
                {(msg.text || (!msg.attachments?.length && !msg.files?.length)) && (
                  <Mrkdwn text={msg.text} ctx={ctx} suffix={msg.edited && <span className="edited">(edited)</span>} />
                )}
                {!!msg.attachments?.length && (
                  <MessageBlocks attachments={msg.attachments} container={container} ctx={ctx} />
                )}
              </>
            )}
          </div>
        )}
        {!!msg.files?.length && <Files files={msg.files} />}
        {msg.failed && msg.client_msg_id && (
          <div className="failed-note">
            Message failed to send.{' '}
            <button
              className="link"
              onClick={() => {
                ws.store.discardFailed(channel, msg.client_msg_id!, msg.thread_ts)
                ws.store.send(channel, msg.text, { thread_ts: msg.thread_ts })
              }}
            >
              Retry
            </button>{' '}
            ·{' '}
            <button className="link" onClick={() => ws.store.discardFailed(channel, msg.client_msg_id!, msg.thread_ts)}>
              Discard
            </button>
          </div>
        )}
        {!!msg.reactions?.length && (
          <div className="reactions">
            {msg.reactions.map((r) => {
              const mine = r.users.includes(me)
              const who = r.users.map((u) => displayName(users[u], u)).join(', ')
              return (
                <button
                  key={r.name}
                  className={`reaction ${mine ? 'mine' : ''}`}
                  title={`${who} reacted with :${r.name}:`}
                  onClick={() =>
                    ws.store
                      .call(mine ? 'reactions.remove' : 'reactions.add', { channel, timestamp: msg.ts, name: r.name })
                      .catch(() => {})
                  }
                >
                  <span className="reaction-emoji">{emojiChar(r.name) ?? `:${r.name}:`}</span>
                  <span className="reaction-count">{r.count}</span>
                </button>
              )
            })}
            <button
              className="reaction add-reaction"
              title="Add reaction"
              onClick={(e) => onReact(e.currentTarget.getBoundingClientRect())}
            >
              ☺︎+
            </button>
          </div>
        )}
        {mode === 'channel' && (msg.reply_count ?? 0) > 0 && (
          <button className="thread-summary" onClick={() => ws.open(channel, msg.ts)}>
            {(msg.reply_users ?? []).slice(0, 3).map((u) => (
              <Avatar key={u} user={users[u]} id={u} size={20} />
            ))}
            <span className="reply-count">
              {msg.reply_count} {msg.reply_count === 1 ? 'reply' : 'replies'}
            </span>
            {msg.latest_reply && <span className="muted">Last reply {relativeTime(msg.latest_reply)}</span>}
          </button>
        )}
      </div>
      {!system && !tombstone && !ephemeral && !msg.pending && !msg.failed && !editing && (
        <div className="message-actions" role="toolbar" aria-label="Message actions">
          <button title="Add reaction" onClick={(e) => onReact(e.currentTarget.getBoundingClientRect())}>
            ☺︎
          </button>
          {mode === 'channel' && (
            <button title="Reply in thread" onClick={() => ws.open(channel, msg.thread_ts ?? msg.ts)}>
              💬︎
            </button>
          )}
          <button title="Copy link" onClick={copyLink}>
            🔗︎
          </button>
          {!msg.subtype || ['thread_broadcast', 'me_message', 'bot_message', 'file_share'].includes(msg.subtype) ? (
            <button
              title={msg.pinned_to?.length ? 'Unpin from channel' : 'Pin to channel'}
              className={msg.pinned_to?.length ? 'on' : ''}
              onClick={() =>
                ws.store
                  .call(msg.pinned_to?.length ? 'pins.remove' : 'pins.add', { channel, timestamp: msg.ts })
                  .catch((e) => alert(`Couldn’t change the pin: ${e.code ?? e}`))
              }
            >
              📌︎
            </button>
          ) : null}
          <ShortcutMenu type="message" channel={channel} messageTs={msg.ts} label="More actions" />
          {canEdit && (
            <button title="Edit message" onClick={() => onEdit(true)}>
              ✎
            </button>
          )}
          {canDelete && (
            <button title="Delete message" className="danger" onClick={del}>
              🗑︎
            </button>
          )}
        </div>
      )}
    </div>
  )
}

function EditBox({ channel, msg, onDone }: { channel: string; msg: Message; onDone: () => void }) {
  const { store } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const groups = useWorkspace(store, (s) => s.usergroups)
  const [text, setText] = useState(() => decodeForEdit(msg.text, users, convs))
  const [busy, setBusy] = useState(false)

  const save = async () => {
    const encoded = encodeOutgoing(text.trim(), users, convs, groups)
    if (!encoded || encoded === msg.text) return onDone()
    setBusy(true)
    try {
      await store.call('chat.update', { channel, ts: msg.ts, text: encoded })
      onDone()
    } catch (e: any) {
      alert(`Couldn’t save: ${e.code ?? e}`)
      setBusy(false)
    }
  }

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') onDone()
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      save()
    }
  }

  return (
    <div className="edit-box">
      <textarea value={text} autoFocus onChange={(e) => setText(e.target.value)} onKeyDown={onKey} rows={Math.min(8, text.split('\n').length + 1)} />
      <div className="edit-actions">
        <button onClick={onDone} disabled={busy}>
          Cancel
        </button>
        <button className="primary-small" onClick={save} disabled={busy}>
          Save
        </button>
      </div>
    </div>
  )
}

const INLINE_IMAGES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp']

/** A file URL relative to this page, so the session cookie goes with it whatever host PUBLIC_URL names. */
function local(url?: string) {
  if (!url) return undefined
  const u = new URL(url)
  return u.pathname + u.search
}

function fileIcon(f: SlackFile) {
  if (f.mimetype?.startsWith('image/')) return '🖼︎'
  if (f.filetype === 'pdf') return '📕︎'
  if (['zip', 'gzip'].includes(f.filetype ?? '')) return '🗜︎'
  if (['binary'].includes(f.filetype ?? '')) return '📦︎'
  return '📄︎'
}

function Files({ files }: { files: SlackFile[] }) {
  return (
    <div className="msg-files">
      {files.map((f) =>
        f.mode === 'tombstone' ? (
          <div key={f.id} className="file-card tombstone">
            This file was deleted.
          </div>
        ) : INLINE_IMAGES.includes(f.mimetype ?? '') ? (
          <a key={f.id} className="file-image" href={local(f.url_private)} target="_blank" rel="noreferrer" title={f.title}>
            <img src={local(f.url_private)} alt={f.alt_txt || f.title || f.name} loading="lazy" />
          </a>
        ) : (
          <a key={f.id} className="file-card" href={local(f.url_private_download)} download={f.name}>
            <span className="file-icon" aria-hidden>
              {fileIcon(f)}
            </span>
            <span className="file-meta">
              <span className="file-title">{f.title || f.name}</span>
              <span className="muted small">
                {f.pretty_type}
                {f.size !== undefined && ` · ${formatSize(f.size)}`}
              </span>
            </span>
            <span className="file-download" aria-hidden>
              ⤓
            </span>
          </a>
        ),
      )}
    </div>
  )
}
