import { useEffect, useMemo, useRef, useState } from 'react'
import { EMOJI_GROUPS, searchEmoji } from './emoji'

const WIDTH = 340
const HEIGHT = 360

/** A searchable emoji grid, anchored near `anchor` (a bottom sheet on small screens). */
export function EmojiPicker({ anchor, onPick, onClose }: { anchor: DOMRect; onPick: (name: string) => void; onClose: () => void }) {
  const [q, setQ] = useState('')
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    // Defer so the click that opened us doesn't close us.
    const t = setTimeout(() => document.addEventListener('mousedown', onDown))
    document.addEventListener('keydown', onKey)
    return () => {
      clearTimeout(t)
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const groups = useMemo(() => (q.trim() ? [{ name: 'Results', emoji: searchEmoji(q.trim(), 120) }] : EMOJI_GROUPS), [q])

  const style =
    innerWidth < 768
      ? undefined
      : {
          left: Math.max(8, Math.min(anchor.right - WIDTH, innerWidth - WIDTH - 8)),
          top: anchor.top - HEIGHT - 8 > 8 ? anchor.top - HEIGHT - 8 : Math.min(anchor.bottom + 8, innerHeight - HEIGHT - 8),
        }

  return (
    <div className="emoji-picker" ref={ref} style={style} role="dialog" aria-label="Pick an emoji">
      <input
        autoFocus
        placeholder="Search emoji"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            const first = groups[0]?.emoji[0]
            if (first) onPick(first.name)
          }
        }}
      />
      <div className="emoji-groups">
        {groups.map((g) => (
          <section key={g.name}>
            <h3>{g.name}</h3>
            <div className="emoji-grid">
              {g.emoji.map((e) => (
                <button key={e.name} title={`:${e.name}:`} onClick={() => onPick(e.name)}>
                  {e.char}
                </button>
              ))}
            </div>
          </section>
        ))}
        {groups[0]?.emoji.length === 0 && <p className="muted small">No emoji match “{q}”.</p>}
      </div>
    </div>
  )
}
