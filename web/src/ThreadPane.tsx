import { useEffect, useMemo, useState } from 'react'
import { Composer, TypingIndicator } from './Composer'
import { useWs } from './context'
import { MessageList } from './MessageList'
import { convName, mergeMessages, useWorkspace } from './store'

export function ThreadPane({ channel, thread }: { channel: string; thread: string }) {
  const { store, open } = useWs()
  const key = `${channel}/${thread}`
  const list = useWorkspace(store, (s) => s.threads[key])
  const ephemerals = useWorkspace(store, (s) => s.ephemerals[channel])
  const msgs = useMemo(
    () => mergeMessages(list?.msgs ?? [], (ephemerals ?? []).filter((m) => m.thread_ts === thread)),
    [list?.msgs, ephemerals, thread],
  )
  const conv = useWorkspace(store, (s) => s.convs[channel])
  const users = useWorkspace(store, (s) => s.users)
  const [error, setError] = useState<string>()

  useEffect(() => {
    if (!list) store.loadThread(channel, thread).catch((e) => setError(e.code ?? 'error'))
  }, [channel, thread, list, store])

  const name = conv ? convName(conv, { users, me: store.state.me }) : ''
  const label = conv && !conv.is_im && !conv.is_mpim ? `#${name}` : name

  return (
    <aside className="thread-pane" aria-label="Thread">
      <header className="main-head">
        <button className="back icon-button" onClick={() => open(channel)} aria-label="Back to channel">
          ‹
        </button>
        <h1>
          Thread <span className="muted thread-where">{label}</span>
        </h1>
        <button className="icon-button close-thread" onClick={() => open(channel)} aria-label="Close thread">
          ×
        </button>
      </header>
      {error ? (
        <div className="pane-loading">{error === 'thread_not_found' ? 'This thread no longer exists.' : `Couldn’t load: ${error}`}</div>
      ) : (
        <>
          <MessageList channel={channel} msgs={msgs} mode="thread" />
          {conv?.is_member && !conv.is_archived && (
            <>
              <Composer channel={channel} thread={thread} placeholder="Reply…" broadcastLabel={`Also send to ${label}`} />
              <TypingIndicator keyName={key} />
            </>
          )}
        </>
      )}
    </aside>
  )
}
