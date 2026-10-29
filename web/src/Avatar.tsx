import type { SlackUser } from './api'
import { emojiChar } from './emoji'

const COLORS = ['#e8912d', '#2bac76', '#1d9bd1', '#cd2553', '#4a154b', '#e01e5a', '#36c5f0', '#9e3997', '#3aa3e3', '#de4e2b']

function hash(s: string) {
  let h = 0
  for (const ch of s) h = (h * 31 + ch.charCodeAt(0)) | 0
  return Math.abs(h)
}

/** Initials on a colour derived from the user ID (no uploaded avatars yet). */
export function Avatar({
  user,
  id,
  size = 36,
  icons,
}: {
  user?: SlackUser
  id: string
  size?: number
  icons?: { emoji?: string; image_48?: string }
}) {
  const style = { width: size, height: size, fontSize: Math.round(size * 0.42), borderRadius: Math.round(size * 0.22) }
  const emoji = icons?.emoji && emojiChar(icons.emoji.replace(/^:|:$/g, ''))
  if (emoji) {
    return (
      <span className="avatar emoji-avatar" style={{ ...style, fontSize: Math.round(size * 0.7) }} aria-hidden>
        {emoji}
      </span>
    )
  }
  if (icons?.image_48 && /^https?:/.test(icons.image_48)) {
    return <img className="avatar" style={style} src={icons.image_48} alt="" />
  }
  const name = user?.profile?.display_name || user?.real_name || user?.name || '?'
  const letters = name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0])
    .join('')
    .toUpperCase()
  return (
    <span className="avatar" style={{ ...style, background: COLORS[hash(id) % COLORS.length] }} aria-hidden>
      {letters || '?'}
    </span>
  )
}
