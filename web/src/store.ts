// Client-side state for one workspace session: users, conversations, unread
// counts, loaded message lists and typing indicators. Kept in a tiny external
// store (read with `useWorkspace(selector)`) and updated by API responses and
// realtime events.

import { useSyncExternalStore } from 'react'
import { slack, slackAll, type Conversation, type Counts, type Message, type SlackUser, type SlashCommand, type View } from './api'

export type MessageList = { msgs: Message[]; hasMore: boolean; cursor?: string; loading: boolean }

export type WorkspaceState = {
  teamId: string
  me: string
  ready: boolean
  users: Record<string, SlackUser>
  /** Conversations the user belongs to, plus any they have previewed. */
  convs: Record<string, Conversation>
  counts: Record<string, Counts>
  channels: Record<string, MessageList>
  /** Keyed by `${channel}/${thread_ts}`. */
  threads: Record<string, MessageList>
  /** channel (or channel/thread_ts) → user → expiry (ms). */
  typing: Record<string, Record<string, number>>
  /** Ephemeral messages per channel (they live only in this client, like Slack's). */
  ephemerals: Record<string, Message[]>
  /** The open modal stack, bottom first. */
  modals: View[]
  /** App Home views by app ID. */
  homeViews: Record<string, View>
}

type Listener = () => void

export class WorkspaceStore {
  state: WorkspaceState
  private listeners = new Set<Listener>()

  constructor(teamId: string, me: string) {
    this.state = {
      teamId,
      me,
      ready: false,
      users: {},
      convs: {},
      counts: {},
      channels: {},
      threads: {},
      typing: {},
      ephemerals: {},
      modals: [],
      homeViews: {},
    }
  }

  subscribe = (l: Listener) => {
    this.listeners.add(l)
    return () => this.listeners.delete(l)
  }

  set(update: (s: WorkspaceState) => Partial<WorkspaceState>) {
    this.state = { ...this.state, ...update(this.state) }
    for (const l of this.listeners) l()
  }

  call<T = Record<string, unknown>>(method: string, args: Record<string, unknown> = {}) {
    return slack<T>(this.state.teamId, method, args)
  }

  /** Loads users, memberships and unread counts. Also used to resync after a reconnect. */
  async load() {
    const t = this.state.teamId
    const [users, convs, counts, views] = await Promise.all([
      slackAll<SlackUser>(t, 'users.list', 'members'),
      slackAll<Conversation>(t, 'users.conversations', 'channels', { types: 'public_channel,private_channel,mpim,im' }),
      this.call<{ channels: Counts[]; mpims: Counts[]; ims: Counts[] }>('client.counts'),
      this.call<{ views: View[] }>('emu.views.current'),
    ])
    this.set(() => ({
      ready: true,
      users: Object.fromEntries(users.map((u) => [u.id, u])),
      convs: Object.fromEntries(convs.map((c) => [c.id, { ...c, is_member: true }])),
      counts: Object.fromEntries([...counts.channels, ...counts.mpims, ...counts.ims].map((c) => [c.id, c])),
      // Loaded histories may be stale; drop them so views refetch.
      channels: {},
      threads: {},
      modals: views.views,
    }))
  }

  private commands?: { list: SlashCommand[]; at: number }

  /** Slash commands from installed apps (cached briefly). */
  async slashCommands(): Promise<SlashCommand[]> {
    if (this.commands && Date.now() - this.commands.at < 15_000) return this.commands.list
    const r = await this.call<{ commands: SlashCommand[] }>('emu.commands.list')
    this.commands = { list: r.commands, at: Date.now() }
    return r.commands
  }

  setHomeView(appId: string, view: View) {
    this.set((s) => ({ homeViews: { ...s.homeViews, [appId]: view } }))
  }

  dismissEphemeral(channel: string, ts: string) {
    this.set((s) => ({
      ephemerals: { ...s.ephemerals, [channel]: (s.ephemerals[channel] ?? []).filter((m) => m.ts !== ts) },
    }))
  }

  async refreshCounts() {
    const counts = await this.call<{ channels: Counts[]; mpims: Counts[]; ims: Counts[] }>('client.counts')
    this.set(() => ({
      counts: Object.fromEntries([...counts.channels, ...counts.mpims, ...counts.ims].map((c) => [c.id, c])),
    }))
  }

  upsertConv(c: Conversation) {
    this.set((s) => ({ convs: { ...s.convs, [c.id]: { ...s.convs[c.id], ...c } } }))
  }

  // ---------- message lists ----------

  async loadHistory(channel: string, older = false) {
    const cur = this.state.channels[channel]
    if (cur?.loading || (older && !cur?.hasMore)) return
    this.patchList('channels', channel, { loading: true })
    try {
      const r = await this.call<{ messages: Message[]; has_more: boolean; response_metadata?: { next_cursor?: string } }>(
        'conversations.history',
        { channel, limit: 50, cursor: older ? cur?.cursor : undefined },
      )
      const fetched = r.messages.slice().reverse()
      this.set((s) => {
        const prev = s.channels[channel]
        const msgs = older && prev ? mergeMessages(fetched, prev.msgs) : mergeMessages(fetched, (prev?.msgs ?? []).filter((m) => m.pending))
        return {
          channels: {
            ...s.channels,
            [channel]: { msgs, hasMore: r.has_more, cursor: r.response_metadata?.next_cursor, loading: false },
          },
        }
      })
    } catch (e) {
      this.patchList('channels', channel, { loading: false })
      throw e
    }
  }

  async loadThread(channel: string, ts: string) {
    const key = `${channel}/${ts}`
    this.patchList('threads', key, { loading: true })
    const msgs = await slackAll<Message>(this.state.teamId, 'conversations.replies', 'messages', { channel, ts })
    this.set((s) => ({ threads: { ...s.threads, [key]: { msgs, hasMore: false, loading: false } } }))
  }

  private patchList(kind: 'channels' | 'threads', key: string, patch: Partial<MessageList>) {
    this.set((s) => ({
      [kind]: { ...s[kind], [key]: { ...(s[kind][key] ?? { msgs: [], hasMore: false, loading: false }), ...patch } },
    }))
  }

  /** Applies `f` to every loaded copy of the message (channel list and thread lists). */
  private mapMessage(channel: string, ts: string, f: (m: Message) => Message | null) {
    this.set((s) => {
      const apply = (list?: MessageList) => {
        if (!list || !list.msgs.some((m) => m.ts === ts)) return list
        return { ...list, msgs: list.msgs.flatMap((m) => (m.ts === ts ? (f(m) ?? []) : [m])) }
      }
      const threads = { ...s.threads }
      for (const key of Object.keys(threads)) {
        if (key.startsWith(channel + '/')) threads[key] = apply(threads[key])!
      }
      const ch = apply(s.channels[channel])
      return { channels: ch ? { ...s.channels, [channel]: ch } : s.channels, threads }
    })
  }

  private addToList(kind: 'channels' | 'threads', key: string, msg: Message) {
    this.set((s) => {
      const list = s[kind][key]
      if (!list) return {}
      // Replace our optimistic copy if this is the confirmed version.
      const rest = list.msgs.filter((m) => m.ts !== msg.ts && !(msg.client_msg_id && m.client_msg_id === msg.client_msg_id))
      return { [kind]: { ...s[kind], [key]: { ...list, msgs: mergeMessages(rest, [msg]) } } }
    })
  }

  // ---------- sending ----------

  async send(channel: string, text: string, opts: { thread_ts?: string; reply_broadcast?: boolean } = {}) {
    const client_msg_id = crypto.randomUUID()
    const pending: Message = {
      ts: `9999999999.${client_msg_id}`,
      user: this.state.me,
      text,
      client_msg_id,
      pending: true,
      thread_ts: opts.thread_ts,
      subtype: opts.reply_broadcast ? 'thread_broadcast' : undefined,
    }
    const key = opts.thread_ts ? `${channel}/${opts.thread_ts}` : channel
    const kind = opts.thread_ts ? 'threads' : 'channels'
    this.addToList(kind, key, pending)
    try {
      const r = await this.call<{ message: Message }>('chat.postMessage', { channel, text, client_msg_id, ...opts })
      this.addToList(kind, key, r.message)
      if (opts.reply_broadcast) this.addToList('channels', channel, r.message)
      if (!opts.thread_ts) this.markRead(channel, r.message.ts)
    } catch {
      this.set((s) => {
        const list = s[kind][key]
        if (!list) return {}
        const msgs = list.msgs.map((m) => (m.client_msg_id === client_msg_id ? { ...m, pending: false, failed: true } : m))
        return { [kind]: { ...s[kind], [key]: { ...list, msgs } } }
      })
    }
  }

  discardFailed(channel: string, client_msg_id: string, thread_ts?: string) {
    const key = thread_ts ? `${channel}/${thread_ts}` : channel
    const kind = thread_ts ? 'threads' : 'channels'
    this.set((s) => {
      const list = s[kind][key]
      if (!list) return {}
      return { [kind]: { ...s[kind], [key]: { ...list, msgs: list.msgs.filter((m) => m.client_msg_id !== client_msg_id) } } }
    })
  }

  // ---------- read state ----------

  private markTimer = new Map<string, number>()

  /** Marks the channel read up to `ts` (debounced server call; immediate locally). */
  markRead(channel: string, ts: string) {
    const c = this.state.counts[channel]
    if (c && c.last_read >= ts && c.unread_count === 0 && c.mention_count === 0) return
    this.set((s) => ({
      counts: {
        ...s.counts,
        [channel]: { ...(s.counts[channel] ?? { id: channel, latest: ts }), last_read: ts, unread_count: 0, mention_count: 0 },
      },
    }))
    clearTimeout(this.markTimer.get(channel))
    this.markTimer.set(
      channel,
      window.setTimeout(() => {
        if (this.state.convs[channel]?.is_member) this.call('conversations.mark', { channel, ts }).catch(() => {})
      }, 400),
    )
  }

  // ---------- realtime ----------

  /** The channel the user is looking at (and whether the window is focused), for read tracking. */
  viewing?: { channel: string; focused: boolean }

  applyEvent(ev: Record<string, any>) {
    const s = this.state
    switch (ev.type) {
      case 'message':
        return this.applyMessageEvent(ev)
      case 'emu_thread_updated':
        return this.mapMessage(ev.channel, ev.message.ts, (m) => ({ ...m, ...ev.message }))
      case 'reaction_added':
      case 'reaction_removed': {
        const added = ev.type === 'reaction_added'
        return this.mapMessage(ev.item.channel, ev.item.ts, (m) => ({
          ...m,
          reactions: applyReaction(m.reactions ?? [], ev.reaction, ev.user, added),
        }))
      }
      case 'channel_joined':
      case 'group_joined':
      case 'im_created':
      case 'mpim_joined':
        this.upsertConv({ ...ev.channel, is_member: true })
        if (!s.counts[ev.channel.id]) {
          this.set((st) => ({
            counts: { ...st.counts, [ev.channel.id]: { id: ev.channel.id, last_read: '0', unread_count: 0, mention_count: 0 } },
          }))
        }
        return
      case 'channel_left':
      case 'group_left':
        if (s.convs[ev.channel]) this.upsertConv({ ...s.convs[ev.channel], is_member: false })
        return
      case 'emu_conversation_updated': {
        const prev = s.convs[ev.channel.id]
        if (prev) this.upsertConv({ ...ev.channel, is_member: prev.is_member, user: prev.user })
        return
      }
      case 'channel_rename':
      case 'group_rename':
        if (s.convs[ev.channel.id]) this.upsertConv({ ...s.convs[ev.channel.id], name: ev.channel.name })
        return
      case 'channel_marked':
      case 'group_marked':
      case 'im_marked': {
        // Read up to the newest message (e.g. from another tab): nothing unread.
        // Otherwise (a stale mark) we can't count locally, so ask the server.
        const c = s.counts[ev.channel]
        if (c && c.latest && ev.ts < c.latest) {
          this.refreshCounts()
        } else {
          this.set((st) => ({
            counts: {
              ...st.counts,
              [ev.channel]: { ...st.counts[ev.channel], id: ev.channel, last_read: ev.ts, unread_count: 0, mention_count: 0 },
            },
          }))
        }
        return
      }
      case 'member_joined_channel':
      case 'member_left_channel': {
        const c = s.convs[ev.channel]
        if (c?.num_members !== undefined) {
          this.upsertConv({ ...c, num_members: c.num_members + (ev.type === 'member_joined_channel' ? 1 : -1) })
        }
        return
      }
      case 'team_join':
      case 'user_change':
        this.set((st) => ({ users: { ...st.users, [ev.user.id]: ev.user } }))
        return
      case 'emu_ephemeral': {
        const msg: Message = { ...ev.message, is_ephemeral: true }
        this.set((st) => {
          const list = (st.ephemerals[ev.channel] ?? []).filter((m) => m.ts !== msg.ts)
          return { ephemerals: { ...st.ephemerals, [ev.channel]: [...list, msg] } }
        })
        return
      }
      case 'emu_ephemeral_deleted':
        return this.dismissEphemeral(ev.channel, ev.ts)
      case 'emu_modals':
        this.set(() => ({ modals: ev.views }))
        return
      case 'emu_home_view':
        this.set((st) => ({ homeViews: { ...st.homeViews, [ev.app_id]: ev.view } }))
        return
      case 'user_typing': {
        const key = ev.thread_ts ? `${ev.channel}/${ev.thread_ts}` : ev.channel
        this.set((st) => ({
          typing: { ...st.typing, [key]: { ...st.typing[key], [ev.user]: Date.now() + 5000 } },
        }))
        return
      }
    }
  }

  private applyMessageEvent(ev: Record<string, any>) {
    const channel: string = ev.channel
    if (ev.subtype === 'message_changed') {
      return this.mapMessage(channel, ev.message.ts, (m) => ({ ...m, ...ev.message, reactions: ev.message.reactions }))
    }
    if (ev.subtype === 'message_deleted') {
      return this.mapMessage(channel, ev.deleted_ts, () => null)
    }
    const msg = stripEventFields(ev)
    const isReply = msg.thread_ts && msg.thread_ts !== msg.ts
    if (isReply) this.addToList('threads', `${channel}/${msg.thread_ts}`, msg)
    if (!isReply || msg.subtype === 'thread_broadcast') this.addToList('channels', channel, msg)

    // Typing stops when someone posts.
    const typingKey = isReply ? `${channel}/${msg.thread_ts}` : channel
    if (msg.user && this.state.typing[typingKey]?.[msg.user]) {
      this.set((st) => {
        const t = { ...st.typing[typingKey] }
        delete t[msg.user!]
        return { typing: { ...st.typing, [typingKey]: t } }
      })
    }

    if (isReply && msg.subtype !== 'thread_broadcast') return
    const mine = msg.user === this.state.me
    const v = this.viewing
    if (mine || (v?.channel === channel && v.focused)) {
      if (!mine) this.markRead(channel, msg.ts)
      return
    }
    const counted = !msg.subtype || ['bot_message', 'thread_broadcast', 'me_message', 'file_share'].includes(msg.subtype)
    if (!counted) return
    const conv = this.state.convs[channel]
    const me = this.state.me
    const mentioned =
      conv?.is_im || msg.text.includes(`<@${me}>`) || /<!(here|channel|everyone)/.test(msg.text)
    this.set((st) => {
      const c = st.counts[channel] ?? { id: channel, last_read: '0', unread_count: 0, mention_count: 0 }
      return {
        counts: {
          ...st.counts,
          [channel]: {
            ...c,
            latest: msg.ts,
            unread_count: c.unread_count + 1,
            mention_count: c.mention_count + (mentioned ? 1 : 0),
          },
        },
      }
    })
  }
}

function stripEventFields(ev: Record<string, any>): Message {
  const { channel: _c, channel_type: _t, event_ts: _e, ...msg } = ev
  return msg as Message
}

/** Merges two ts-sorted lists, de-duplicating by ts (later list wins). */
export function mergeMessages(a: Message[], b: Message[]): Message[] {
  const byTs = new Map<string, Message>()
  for (const m of a) byTs.set(m.ts, m)
  for (const m of b) byTs.set(m.ts, m)
  return [...byTs.values()].sort((x, y) => (x.ts < y.ts ? -1 : x.ts > y.ts ? 1 : 0))
}

function applyReaction(list: Message['reactions'] & {}, name: string, user: string, added: boolean) {
  const out = list.map((r) => ({ ...r, users: [...r.users] }))
  const r = out.find((x) => x.name === name)
  if (added) {
    if (r) {
      if (!r.users.includes(user)) r.users.push(user)
    } else out.push({ name, users: [user], count: 1 })
  } else if (r) {
    r.users = r.users.filter((u) => u !== user)
  }
  return out.map((x) => ({ ...x, count: x.users.length })).filter((x) => x.count > 0)
}

export function useWorkspace<T>(store: WorkspaceStore, select: (s: WorkspaceState) => T): T {
  return useSyncExternalStore(store.subscribe, () => select(store.state))
}

/** A conversation's display name from the viewer's point of view. */
export function convName(c: Conversation | undefined, s: Pick<WorkspaceState, 'users' | 'me'>): string {
  if (!c) return ''
  if (c.is_im) {
    const u = s.users[c.user ?? '']
    const name = u ? u.profile?.display_name || u.real_name || u.name : c.user ?? 'unknown'
    return c.user === s.me ? `${name} (you)` : name
  }
  if (c.is_mpim) {
    // mpdm-alice--bob--carol-1 → alice, bob, carol (minus me)
    const myHandle = s.users[s.me]?.name
    const handles = (c.name ?? '').replace(/^mpdm-/, '').replace(/-\d+$/, '').split('--')
    return handles.filter((h) => h !== myHandle).join(', ')
  }
  return c.name ?? c.id
}
