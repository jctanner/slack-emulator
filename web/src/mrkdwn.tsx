// Slack `mrkdwn` → React, plus the reverse direction for the composer
// (plain typing with @handles and #channels → Slack's wire format).

import { Fragment, type ReactNode } from 'react'
import type { Conversation, SlackUser } from './api'
import { emojiChar } from './emoji'

export type MrkdwnContext = {
  me: string
  users: Record<string, SlackUser>
  convs: Record<string, Conversation>
  onChannel?: (id: string) => void
}

export function displayName(u: SlackUser | undefined, fallback = 'unknown'): string {
  if (!u) return fallback
  return u.profile?.display_name || u.real_name || u.name
}

export function decodeEntities(s: string): string {
  return s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&')
}

function escapeEntities(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

const SAFE_URL = /^(https?:|mailto:)/i

type Rule = { re: RegExp; render: (m: RegExpExecArray, ctx: MrkdwnContext, key: string) => ReactNode }

const rules: Rule[] = [
  { re: /`([^`\n]+)`/, render: (m, _c, k) => <code key={k}>{decodeEntities(m[1])}</code> },
  { re: /<([^<>\n]+)>/, render: (m, c, k) => angle(m[1], c, k) },
  {
    re: /(?<![\p{L}\p{N}*])\*(?=\S)([^*\n]*?\S)\*(?![\p{L}\p{N}*])/u,
    render: (m, c, k) => <strong key={k}>{inline(m[1], c, k)}</strong>,
  },
  {
    re: /(?<![\p{L}\p{N}_])_(?=\S)([^_\n]*?\S)_(?![\p{L}\p{N}_])/u,
    render: (m, c, k) => <em key={k}>{inline(m[1], c, k)}</em>,
  },
  {
    re: /(?<![\p{L}\p{N}~])~(?=\S)([^~\n]*?\S)~(?![\p{L}\p{N}~])/u,
    render: (m, c, k) => <s key={k}>{inline(m[1], c, k)}</s>,
  },
  {
    re: /:([a-z0-9_+'-]+(?:::skin-tone-[2-6])?):/,
    render: (m, _c, k) => {
      const ch = emojiChar(m[1])
      return ch ? (
        <span key={k} className="emoji" title={`:${m[1]}:`}>
          {ch}
        </span>
      ) : (
        m[0]
      )
    },
  },
  {
    re: /\bhttps?:\/\/[^\s<>"]*[^\s<>".,;:!?)\]'*_~]/,
    render: (m, _c, k) => {
      const url = decodeEntities(m[0])
      return (
        <a key={k} href={url} target="_blank" rel="noreferrer noopener">
          {url}
        </a>
      )
    },
  },
]

/** `<...>` tokens: mentions, channel links, special mentions and links. */
function angle(body: string, ctx: MrkdwnContext, key: string): ReactNode {
  const [target, label] = splitOnce(body, '|')
  if (target.startsWith('@')) {
    const id = target.slice(1)
    const name = ctx.users[id] ? displayName(ctx.users[id]) : label ?? id
    return (
      <span key={key} className={`mention ${id === ctx.me ? 'me' : ''}`}>
        @{name}
      </span>
    )
  }
  if (target.startsWith('#')) {
    const id = target.slice(1)
    const name = ctx.convs[id]?.name ?? label ?? id
    return (
      <button key={key} type="button" className="channel-link" onClick={() => ctx.onChannel?.(id)}>
        #{name}
      </button>
    )
  }
  if (target.startsWith('!')) {
    const special = target.slice(1)
    if (['here', 'channel', 'everyone'].includes(special)) {
      return (
        <span key={key} className="mention me">
          @{special}
        </span>
      )
    }
    if (special.startsWith('subteam^')) {
      return (
        <span key={key} className="mention">
          {label ?? '@group'}
        </span>
      )
    }
    return decodeEntities(label ?? special)
  }
  const url = decodeEntities(target)
  if (!SAFE_URL.test(url)) return decodeEntities(label ?? target)
  return (
    <a key={key} href={url} target="_blank" rel="noreferrer noopener">
      {decodeEntities(label ?? target.replace(/^mailto:/, ''))}
    </a>
  )
}

function splitOnce(s: string, sep: string): [string, string | undefined] {
  const i = s.indexOf(sep)
  return i < 0 ? [s, undefined] : [s.slice(0, i), s.slice(i + 1)]
}

/** Inline formatting within one line. */
function inline(text: string, ctx: MrkdwnContext, keyPrefix: string): ReactNode[] {
  const out: ReactNode[] = []
  let rest = text
  let n = 0
  while (rest) {
    let best: { m: RegExpExecArray; rule: Rule } | undefined
    for (const rule of rules) {
      const m = rule.re.exec(rest)
      if (m && (!best || m.index < best.m.index)) best = { m, rule }
    }
    if (!best) {
      out.push(decodeEntities(rest))
      break
    }
    if (best.m.index > 0) out.push(decodeEntities(rest.slice(0, best.m.index)))
    out.push(best.rule.render(best.m, ctx, `${keyPrefix}.${n++}`))
    rest = rest.slice(best.m.index + best.m[0].length)
  }
  return out
}

function lines(text: string, ctx: MrkdwnContext, keyPrefix: string): ReactNode[] {
  const out: ReactNode[] = []
  const all = text.split('\n')
  let i = 0
  while (i < all.length) {
    const isQuote = (l: string) => /^(&gt;|>)\s?/.test(l)
    if (isQuote(all[i])) {
      const quoted: string[] = []
      while (i < all.length && isQuote(all[i])) quoted.push(all[i++].replace(/^(&gt;|>)\s?/, ''))
      out.push(
        <blockquote key={`${keyPrefix}.q${i}`}>{lines(quoted.join('\n'), ctx, `${keyPrefix}.q${i}`)}</blockquote>,
      )
      continue
    }
    if (out.length > 0 && !isQuoteNode(out[out.length - 1])) out.push(<br key={`${keyPrefix}.br${i}`} />)
    out.push(...inline(all[i], ctx, `${keyPrefix}.l${i}`))
    i++
  }
  return out
}

function isQuoteNode(n: ReactNode): boolean {
  return typeof n === 'object' && n !== null && 'type' in n && n.type === 'blockquote'
}

/** Renders a message's `text`. */
export function Mrkdwn({ text, ctx, suffix }: { text: string; ctx: MrkdwnContext; suffix?: ReactNode }) {
  const parts: ReactNode[] = []
  const fence = /```([\s\S]*?)```/g
  let last = 0
  let m: RegExpExecArray | null
  let k = 0
  while ((m = fence.exec(text))) {
    if (m.index > last) parts.push(...lines(text.slice(last, m.index).replace(/\n$/, ''), ctx, `s${k++}`))
    parts.push(<pre key={`pre${k++}`}>{decodeEntities(m[1].replace(/^\n/, ''))}</pre>)
    last = m.index + m[0].length
  }
  if (last < text.length) parts.push(...lines(text.slice(last).replace(/^\n/, ''), ctx, `s${k++}`))
  if (suffix) parts.push(<Fragment key="suffix">{suffix}</Fragment>)
  const jumbo = /^(\s*:[a-z0-9_+'-]+(::skin-tone-[2-6])?:\s*){1,23}$/.test(text) && [...text.matchAll(/:([^:\s]+(?:::skin-tone-[2-6])?):/g)].every((x) => emojiChar(x[1]))
  return <div className={`mrkdwn ${jumbo ? 'jumbo' : ''}`}>{parts}</div>
}

/** Plain-text preview (for notifications, titles, the sidebar). */
export function plainText(text: string, ctx: Pick<MrkdwnContext, 'users' | 'convs'>): string {
  return decodeEntities(
    text.replace(/<([^<>]+)>/g, (_, body: string) => {
      const [t, label] = splitOnce(body, '|')
      if (t.startsWith('@')) return '@' + (ctx.users[t.slice(1)] ? displayName(ctx.users[t.slice(1)]) : label ?? t.slice(1))
      if (t.startsWith('#')) return '#' + (ctx.convs[t.slice(1)]?.name ?? label ?? t.slice(1))
      if (t.startsWith('!')) return '@' + (label ?? t.slice(1))
      return label ?? t
    }),
  )
}

/** Composer text → Slack wire format: escapes `&<>` and links @handles / #channels. */
export function encodeOutgoing(text: string, users: Record<string, SlackUser>, convs: Record<string, Conversation>): string {
  const byHandle = new Map(Object.values(users).map((u) => [u.name.toLowerCase(), u.id]))
  const byChannel = new Map(
    Object.values(convs)
      .filter((c) => c.name && !c.is_im && !c.is_mpim)
      .map((c) => [c.name!, c.id]),
  )
  return escapeEntities(text)
    .replace(/(^|[\s(])@([\p{L}\p{N}._-]+)/gu, (all, pre: string, handle: string) => {
      const h = handle.replace(/[.]+$/, '')
      const tail = handle.slice(h.length)
      if (['here', 'channel', 'everyone'].includes(h)) return `${pre}<!${h}>${tail}`
      const id = byHandle.get(h.toLowerCase())
      return id ? `${pre}<@${id}>${tail}` : all
    })
    .replace(/(^|[\s(])#([\p{L}\p{N}_-]+)/gu, (all, pre: string, name: string) => {
      const id = byChannel.get(name)
      return id ? `${pre}<#${id}|${name}>` : all
    })
}

/** Slack wire format → editable composer text (inverse of `encodeOutgoing`). */
export function decodeForEdit(text: string, users: Record<string, SlackUser>, convs: Record<string, Conversation>): string {
  return decodeEntities(
    text.replace(/<([^<>]+)>/g, (all, body: string) => {
      const [t, label] = splitOnce(body, '|')
      if (t.startsWith('@')) return '@' + (users[t.slice(1)]?.name ?? label ?? t.slice(1))
      if (t.startsWith('#')) return '#' + (convs[t.slice(1)]?.name ?? label ?? t.slice(1))
      if (t.startsWith('!')) return '@' + t.slice(1).split('^')[0]
      if (label) return all
      return t
    }),
  )
}
