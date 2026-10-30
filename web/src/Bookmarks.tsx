// The bookmarks bar under a channel's header (bookmarks.*).

import { useEffect, useState, type FormEvent } from 'react'
import { ApiError } from './api'
import { useWs } from './context'
import { emojiChar } from './emoji'
import { useWorkspace } from './store'

type Bookmark = { id: string; title: string; link: string; emoji?: string | null }

export function BookmarksBar({ channel, canEdit }: { channel: string; canEdit: boolean }) {
  const { store } = useWs()
  const version = useWorkspace(store, (s) => s.bookmarksVersion[channel] ?? 0)
  const [bookmarks, setBookmarks] = useState<Bookmark[]>([])
  const [adding, setAdding] = useState(false)

  useEffect(() => {
    store
      .call<{ bookmarks: Bookmark[] }>('bookmarks.list', { channel_id: channel })
      .then((r) => setBookmarks(r.bookmarks), () => setBookmarks([]))
  }, [channel, store, version])

  if (!bookmarks.length && !canEdit) return null
  const icon = (b: Bookmark) => (b.emoji ? (emojiChar(b.emoji.replace(/:/g, '')) ?? '🔗︎') : '🔗︎')

  return (
    <div className="bookmarks-bar">
      {bookmarks.map((b) => (
        <span key={b.id} className="bookmark">
          <a href={b.link} target="_blank" rel="noreferrer noopener" title={b.link}>
            <span aria-hidden>{icon(b)}</span> {b.title}
          </a>
          {canEdit && (
            <button
              className="bookmark-remove"
              aria-label={`Remove ${b.title}`}
              onClick={() => store.call('bookmarks.remove', { channel_id: channel, bookmark_id: b.id }).catch(() => {})}
            >
              ×
            </button>
          )}
        </span>
      ))}
      {canEdit &&
        (adding ? (
          <AddBookmark channel={channel} onDone={() => setAdding(false)} />
        ) : (
          <button className="bookmark-add" onClick={() => setAdding(true)}>
            + Add a bookmark
          </button>
        ))}
    </div>
  )
}

function AddBookmark({ channel, onDone }: { channel: string; onDone: () => void }) {
  const { store } = useWs()
  const [link, setLink] = useState('')
  const [title, setTitle] = useState('')
  const [error, setError] = useState<string>()

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    try {
      await store.call('bookmarks.add', { channel_id: channel, type: 'link', link, title: title || link })
      onDone()
    } catch (err) {
      setError(err instanceof ApiError && err.code === 'invalid_link' ? 'Use an http(s) link.' : 'Couldn’t add it.')
    }
  }

  return (
    <form className="bookmark-form" onSubmit={submit}>
      <input autoFocus type="url" placeholder="https://…" value={link} onChange={(e) => setLink(e.target.value)} aria-label="Link" required />
      <input placeholder="Name (optional)" value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Name" />
      <button type="submit" className="primary-small">
        Add
      </button>
      <button type="button" className="bookmark-cancel" onClick={onDone}>
        Cancel
      </button>
      {error && <span className="error small">{error}</span>}
    </form>
  )
}
