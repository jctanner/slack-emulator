// Scheduling messages from the composer (chat.scheduleMessage), and the list
// of this conversation's pending ones above it.

import { useEffect, useRef, useState } from 'react'
import { useWs } from './context'
import { useWorkspace } from './store'

type Pending = { id: string; post_at: number; text: string }

function at(hours: number, minutes = 0, dayOffset = 0) {
  const d = new Date()
  d.setDate(d.getDate() + dayOffset)
  d.setHours(hours, minutes, 0, 0)
  return Math.floor(d.getTime() / 1000)
}

function formatWhen(unix: number) {
  const d = new Date(unix * 1000)
  const today = new Date().toDateString() === d.toDateString()
  return today
    ? `today at ${d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`
    : d.toLocaleString([], { weekday: 'short', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

/** The composer's "Schedule message" button and menu. */
export function SchedulePicker({ disabled, onSchedule }: { disabled: boolean; onSchedule: (postAt: number) => Promise<void> }) {
  const [open, setOpen] = useState(false)
  const [custom, setCustom] = useState('')
  const [error, setError] = useState<string>()
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])

  const choose = async (postAt: number) => {
    setError(undefined)
    try {
      await onSchedule(postAt)
      setOpen(false)
      setCustom('')
    } catch (e: any) {
      setError(e?.code === 'time_in_past' ? 'Pick a time in the future.' : `Couldn’t schedule it (${e?.code ?? 'error'}).`)
    }
  }

  const now = Math.floor(Date.now() / 1000)
  const presets: [string, number][] = [
    ['In 1 minute', now + 60],
    ['In 30 minutes', now + 1800],
    ['Tomorrow at 9:00 AM', at(9, 0, 1)],
  ]
  return (
    <div className="shortcut-menu schedule-menu" ref={ref}>
      <button
        type="button"
        className="schedule-button"
        title="Schedule message"
        aria-label="Schedule message"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        🕒︎
      </button>
      {open && (
        <div className="bk-menu schedule-popover" role="menu">
          <div className="schedule-title">Schedule message</div>
          {presets.map(([label, t]) => (
            <button key={label} type="button" role="menuitem" onClick={() => choose(t)}>
              {label}
            </button>
          ))}
          <form
            className="schedule-custom"
            onSubmit={(e) => {
              e.preventDefault()
              if (custom) choose(Math.floor(new Date(custom).getTime() / 1000))
            }}
          >
            <input type="datetime-local" value={custom} onChange={(e) => setCustom(e.target.value)} aria-label="Custom time" />
            <button type="submit" className="primary-small" disabled={!custom}>
              Schedule
            </button>
          </form>
          {error && <div className="bk-options-status error">{error}</div>}
        </div>
      )}
    </div>
  )
}

/** "2 scheduled messages" above the composer, expandable to cancel them. */
export function ScheduledList({ channel }: { channel: string }) {
  const { store } = useWs()
  const version = useWorkspace(store, (s) => s.scheduledVersion)
  const [items, setItems] = useState<Pending[]>([])
  const [open, setOpen] = useState(false)

  useEffect(() => {
    let stopped = false
    const load = () =>
      store
        .call<{ scheduled_messages: Pending[] }>('chat.scheduledMessages.list', { channel })
        .then((r) => !stopped && setItems(r.scheduled_messages), () => {})
    load()
    // They disappear as they're posted.
    const t = setInterval(load, 10_000)
    return () => {
      stopped = true
      clearInterval(t)
    }
  }, [channel, store, version])

  if (!items.length) return null
  return (
    <div className="scheduled-bar">
      <button className="link" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        🕒︎ {items.length} scheduled {items.length === 1 ? 'message' : 'messages'}
      </button>
      {open && (
        <ul className="scheduled-list">
          {items.map((m) => (
            <li key={m.id}>
              <span className="muted small">{formatWhen(m.post_at)}</span>
              <span className="scheduled-text">{m.text || '(blocks)'}</span>
              <button
                aria-label="Cancel scheduled message"
                title="Cancel"
                onClick={() =>
                  store
                    .call('chat.deleteScheduledMessage', { channel, scheduled_message_id: m.id })
                    .then(() => store.scheduledChanged(), () => {})
                }
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
