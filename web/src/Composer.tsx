import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import type { Conversation, SlackUser, SlashCommand } from './api'
import { useWs } from './context'
import { searchEmoji } from './emoji'
import { displayName, encodeOutgoing } from './mrkdwn'
import { useWorkspace } from './store'
import { Avatar } from './Avatar'

// Unsent text per conversation/thread, kept while the app is open.
const drafts = new Map<string, string>()

type Suggestion = { key: string; insert: string; label: React.ReactNode; detail?: string }

const TOUCH = matchMedia('(pointer: coarse)').matches

/** Built-in commands handled by the client. */
const BUILTIN_COMMANDS: SlashCommand[] = [
  { command: '/me', description: 'Display action text', usage_hint: '[your text]', app_id: '', app_name: 'Slack' },
]

/** `/command rest`, unless the message starts with a space or `//`. */
export function parseCommand(text: string): { command: string; text: string } | null {
  const m = /^\/([a-z0-9_-]+)(?:\s+([\s\S]*))?$/i.exec(text)
  return m ? { command: `/${m[1].toLowerCase()}`, text: (m[2] ?? '').trim() } : null
}

/** Finds an autocomplete trigger (`@`, `#`, `:`, or `/` at the start) just before the caret. */
function triggerAt(text: string, caret: number) {
  const cmd = /^\/([a-z0-9_-]*)$/i.exec(text.slice(0, caret))
  if (cmd) return { char: '/', query: cmd[1].toLowerCase(), start: 0 }
  const m = /(^|[\s(])([@#:])([\p{L}\p{N}._+'-]*)$/u.exec(text.slice(0, caret))
  if (!m) return null
  return { char: m[2], query: m[3].toLowerCase(), start: caret - m[3].length - 1 }
}

function suggestions(
  trigger: NonNullable<ReturnType<typeof triggerAt>>,
  users: Record<string, SlackUser>,
  convs: Record<string, Conversation>,
  commands: SlashCommand[],
): Suggestion[] {
  const q = trigger.query
  if (trigger.char === '/') {
    return [...BUILTIN_COMMANDS, ...commands]
      .filter((c) => c.command.slice(1).startsWith(q))
      .sort((a, b) => a.command.localeCompare(b.command))
      .slice(0, 8)
      .map((c) => ({
        key: `${c.app_id}${c.command}`,
        insert: `${c.command} `,
        label: (
          <>
            <strong>{c.command}</strong>
            {c.usage_hint && <span className="muted"> {c.usage_hint}</span>}
            {c.description && <span className="ac-desc"> {c.description}</span>}
          </>
        ),
        detail: c.app_name,
      }))
  }
  if (trigger.char === '@') {
    const people = Object.values(users)
      .filter((u) => !u.deleted)
      .filter((u) => [u.name, u.real_name, u.profile?.display_name].some((s) => s?.toLowerCase().includes(q)))
      .sort((a, b) => Number(!a.name.startsWith(q)) - Number(!b.name.startsWith(q)) || a.name.localeCompare(b.name))
      .slice(0, 8)
      .map((u) => ({
        key: u.id,
        insert: `@${u.name} `,
        label: (
          <>
            <Avatar user={u} id={u.id} size={20} /> <strong>{displayName(u)}</strong>
          </>
        ),
        detail: `@${u.name}`,
      }))
    const special = ['here', 'channel', 'everyone']
      .filter((s) => s.startsWith(q))
      .map((s) => ({ key: s, insert: `@${s} `, label: <strong>@{s}</strong>, detail: s === 'here' ? 'Notify active members' : 'Notify everyone in this channel' }))
    return [...people, ...special].slice(0, 8)
  }
  if (trigger.char === '#') {
    return Object.values(convs)
      .filter((c) => c.name && !c.is_im && !c.is_mpim && !c.is_archived && c.name.includes(q))
      .sort((a, b) => Number(!a.name!.startsWith(q)) - Number(!b.name!.startsWith(q)) || a.name!.localeCompare(b.name!))
      .slice(0, 8)
      .map((c) => ({ key: c.id, insert: `#${c.name} `, label: <strong>{c.is_private ? '🔒︎' : '#'} {c.name}</strong> }))
  }
  if (q.length < 2) return []
  return searchEmoji(q, 8).map((e) => ({
    key: e.name,
    insert: `:${e.name}: `,
    label: (
      <>
        <span className="emoji">{e.char}</span> :{e.name}:
      </>
    ),
  }))
}

export function Composer({
  channel,
  thread,
  placeholder,
  broadcastLabel,
}: {
  channel: string
  thread?: string
  placeholder: string
  /** In a thread: label for the "also send to channel" checkbox. */
  broadcastLabel?: string
}) {
  const { store, sendTyping, typingDone } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const draftKey = thread ? `${channel}/${thread}` : channel
  const [text, setText] = useState(() => drafts.get(draftKey) ?? '')
  const [caret, setCaret] = useState(0)
  const [active, setActive] = useState(0)
  const [dismissed, setDismissed] = useState<number | null>(null)
  const [broadcast, setBroadcast] = useState(false)
  const [commands, setCommands] = useState<SlashCommand[]>([])
  const [attachments, setAttachments] = useState<File[]>([])
  const [uploading, setUploading] = useState(false)
  const [uploadError, setUploadError] = useState<string>()
  const [dragging, setDragging] = useState(false)
  const filePicker = useRef<HTMLInputElement>(null)
  const addFiles = (list: FileList | File[] | null) => {
    if (!list?.length) return
    setUploadError(undefined)
    setAttachments((a) => [...a, ...Array.from(list)].slice(0, 10))
  }
  const ref = useRef<HTMLTextAreaElement>(null)
  const typingCommand = text.startsWith('/')

  useEffect(() => {
    if (typingCommand) store.slashCommands().then(setCommands, () => {})
  }, [typingCommand, store])

  useEffect(() => {
    drafts.set(draftKey, text)
  }, [draftKey, text])

  // Grow with content up to a limit.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`
  }, [text])

  useEffect(() => {
    if (!TOUCH) ref.current?.focus()
  }, [draftKey])

  const trigger = triggerAt(text, caret)
  const items = useMemo(
    () => (trigger && trigger.start !== dismissed ? suggestions(trigger, users, convs, commands) : []),
    [trigger?.char, trigger?.query, trigger?.start, dismissed, users, convs, commands],
  )

  const pick = (s: Suggestion) => {
    if (!trigger) return
    const next = text.slice(0, trigger.start) + s.insert + text.slice(caret)
    const pos = trigger.start + s.insert.length
    setText(next)
    setCaret(pos)
    pendingCaret.current = pos
  }

  // Place the caret after an inserted suggestion before anything else is typed.
  const pendingCaret = useRef<number | null>(null)
  useLayoutEffect(() => {
    const pos = pendingCaret.current
    if (pos === null) return
    pendingCaret.current = null
    ref.current?.focus()
    ref.current?.setSelectionRange(pos, pos)
  }, [text])

  const submit = () => {
    const trimmed = text.trim()
    if (uploading) return
    if (attachments.length) return sendFiles(trimmed)
    if (!trimmed) return
    const cmd = parseCommand(text.trimEnd())
    if (cmd) {
      if (cmd.command === '/me') {
        store.call('chat.meMessage', { channel, text: encodeOutgoing(cmd.text, users, convs) }).catch(() => {})
      } else {
        // The server answers unknown commands and app failures with a Slackbot message.
        store.call('chat.command', { channel, command: cmd.command, text: cmd.text }).catch(() => {})
      }
      setText('')
      typingDone(channel, thread)
      return
    }
    store.send(channel, encodeOutgoing(trimmed, users, convs), {
      thread_ts: thread,
      reply_broadcast: thread && broadcast ? true : undefined,
    })
    setText('')
    setBroadcast(false)
    typingDone(channel, thread)
  }

  const sendFiles = async (trimmed: string) => {
    setUploading(true)
    setUploadError(undefined)
    try {
      await store.sendFiles(channel, encodeOutgoing(trimmed, users, convs), attachments, { thread_ts: thread })
      setAttachments([])
      setText('')
      typingDone(channel, thread)
    } catch (e: any) {
      const detail = e?.data?.response_metadata?.messages?.[0]?.replace('[ERROR] ', '')
      setUploadError(detail ?? e?.message ?? 'Upload failed')
    } finally {
      setUploading(false)
    }
  }

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (items.length > 0) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((a) => (a + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length)
        return
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault()
        pick(items[Math.min(active, items.length - 1)])
        return
      }
      if (e.key === 'Escape') {
        setDismissed(trigger?.start ?? null)
        return
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !TOUCH && !e.nativeEvent.isComposing) {
      e.preventDefault()
      submit()
    }
  }

  return (
    <div className="composer">
      {items.length > 0 && (
        <ul className="autocomplete" role="listbox">
          {items.map((s, i) => (
            <li
              key={s.key}
              role="option"
              aria-selected={i === active}
              className={i === active ? 'active' : ''}
              onMouseDown={(e) => {
                e.preventDefault()
                pick(s)
              }}
              onMouseEnter={() => setActive(i)}
            >
              <span className="ac-label">{s.label}</span>
              {s.detail && <span className="muted small">{s.detail}</span>}
            </li>
          ))}
        </ul>
      )}
      <div
        className={`composer-box ${dragging ? 'dragging' : ''}`}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files')) {
            e.preventDefault()
            setDragging(true)
          }
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          if (!e.dataTransfer.files.length) return
          e.preventDefault()
          setDragging(false)
          addFiles(e.dataTransfer.files)
        }}
      >
        {attachments.length > 0 && (
          <ul className="attachments" aria-label="Files to send">
            {attachments.map((f, i) => (
              <li key={`${f.name}-${i}`} className="attachment-chip">
                <span className="attachment-name">{f.name}</span>
                <span className="muted small">{formatSize(f.size)}</span>
                <button
                  aria-label={`Remove ${f.name}`}
                  disabled={uploading}
                  onClick={() => setAttachments((a) => a.filter((_, j) => j !== i))}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
        {uploadError && <div className="failed-note">{uploadError}</div>}
        <textarea
          ref={ref}
          rows={1}
          value={text}
          placeholder={placeholder}
          aria-label={placeholder}
          onChange={(e) => {
            setText(e.target.value)
            setCaret(e.target.selectionStart)
            setActive(0)
            if (e.target.value) sendTyping(channel, thread)
          }}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
          onPaste={(e) => {
            if (e.clipboardData.files.length) {
              e.preventDefault()
              addFiles(e.clipboardData.files)
            }
          }}
        />
        <div className="composer-bar">
          <input
            ref={filePicker}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              addFiles(e.target.files)
              e.target.value = ''
            }}
          />
          <button className="attach" onClick={() => filePicker.current?.click()} aria-label="Attach files" title="Attach files">
            📎︎
          </button>
          {broadcastLabel ? (
            <label className="broadcast">
              <input type="checkbox" checked={broadcast} onChange={(e) => setBroadcast(e.target.checked)} />
              {broadcastLabel}
            </label>
          ) : (
            <span className="muted small composer-hint">{TOUCH ? '' : 'Shift + Enter for a new line'}</span>
          )}
          <button
            className="send"
            onClick={submit}
            disabled={uploading || (!text.trim() && !attachments.length)}
            aria-label={uploading ? 'Uploading' : 'Send'}
          >
            {uploading ? '…' : '➤'}
          </button>
        </div>
      </div>
    </div>
  )
}

/** "alice is typing…" for a conversation or thread. */
export function TypingIndicator({ keyName }: { keyName: string }) {
  const { store } = useWs()
  const typing = useWorkspace(store, (s) => s.typing[keyName])
  const users = useWorkspace(store, (s) => s.users)
  const [, tick] = useState(0)
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 1000)
    return () => clearInterval(t)
  }, [])
  const now = Date.now()
  const who = Object.entries(typing ?? {})
    .filter(([u, exp]) => exp > now && u !== store.state.me)
    .map(([u]) => displayName(users[u], u))
  const text =
    who.length === 0 ? '' : who.length === 1 ? `${who[0]} is typing…` : who.length === 2 ? `${who[0]} and ${who[1]} are typing…` : 'Several people are typing…'
  return <div className="typing" aria-live="polite">{text}</div>
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}
