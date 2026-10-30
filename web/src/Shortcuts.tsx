// Shortcuts from apps: global ones in the composer's ⚡ menu, message ones in
// a message's "More actions" menu. Running one sends the app a `shortcut` or
// `message_action` payload, usually answered by opening a modal.

import { useEffect, useRef, useState } from 'react'
import { ApiError, type Shortcut } from './api'
import { useWs } from './context'
import { useWorkspace } from './store'

/** A button that opens a menu of shortcuts of one type. Renders nothing if there are none. */
export function ShortcutMenu({
  type,
  channel,
  messageTs,
  className,
  label,
}: {
  type: 'global' | 'message'
  channel?: string
  messageTs?: string
  className?: string
  label: string
}) {
  const { store } = useWs()
  const [shortcuts, setShortcuts] = useState<Shortcut[]>([])
  const [open, setOpen] = useState(false)
  const [error, setError] = useState<string>()
  const ref = useRef<HTMLDivElement>(null)
  const appsVersion = useWorkspace(store, (s) => s.appsVersion)

  useEffect(() => {
    store.shortcuts().then((all) => setShortcuts(all.filter((s) => s.type === type)), () => {})
  }, [store, type, open, appsVersion])

  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])

  if (!shortcuts.length) return null

  const run = async (s: Shortcut) => {
    setError(undefined)
    try {
      await store.call('emu.shortcuts.run', {
        app_id: s.app_id,
        callback_id: s.callback_id,
        channel,
        message_ts: messageTs,
      })
      setOpen(false)
    } catch (e) {
      setError(e instanceof ApiError && e.code === 'dispatch_failed' ? `${s.app_name} didn’t respond (${e.data.detail})` : 'Couldn’t run that shortcut')
    }
  }

  return (
    <div className={`shortcut-menu ${className ?? ''}`} ref={ref}>
      <button type="button" title={label} aria-label={label} aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {type === 'global' ? '⚡︎' : '⋯'}
      </button>
      {open && (
        <ul className="bk-menu shortcut-list" role="menu">
          {shortcuts.map((s) => (
            <li key={`${s.app_id}/${s.callback_id}`}>
              <button type="button" role="menuitem" onClick={() => run(s)}>
                <span className="shortcut-name">{s.name}</span>
                <span className="shortcut-desc">
                  {s.description} · {s.app_name}
                </span>
              </button>
            </li>
          ))}
          {error && <li className="bk-options-status error">{error}</li>}
        </ul>
      )}
    </div>
  )
}
