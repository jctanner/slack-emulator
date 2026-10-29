// Thin client for the emulator's browser auth endpoints (/auth/*) and the
// Slack Web API (/api/*), which the UI calls with its session cookie.

export type Team = { id: string; name: string; domain: string; is_member?: boolean }

export type SlackUser = {
  id: string
  team_id: string
  name: string
  real_name: string
  is_admin: boolean
  is_owner: boolean
  is_bot: boolean
  deleted?: boolean
  profile: { real_name: string; display_name: string; email?: string | null }
}

export type Membership = { team: Team; user: SlackUser }

export type Topic = { value: string; creator: string; last_set: number }

export type Conversation = {
  id: string
  name?: string
  is_channel?: boolean
  is_im?: boolean
  is_mpim?: boolean
  is_private?: boolean
  is_archived?: boolean
  is_general?: boolean
  is_member?: boolean
  num_members?: number
  created: number
  creator?: string
  /** DM peer. */
  user?: string
  topic?: Topic
  purpose?: Topic
}

export type Reaction = { name: string; users: string[]; count: number }

export type Message = {
  ts: string
  type?: string
  user?: string
  text: string
  subtype?: string
  thread_ts?: string
  reply_count?: number
  reply_users?: string[]
  latest_reply?: string
  reactions?: Reaction[]
  edited?: { user: string; ts: string }
  bot_id?: string
  username?: string
  icons?: { emoji?: string; image_48?: string }
  app_id?: string
  blocks?: Block[]
  attachments?: Attachment[]
  client_msg_id?: string
  parent_user_id?: string
  hidden?: boolean
  /** Shown only to this user, until reload (`chat.postEphemeral`, slash command replies). */
  is_ephemeral?: boolean
  /** Client-side: sent but not yet confirmed, or failed to send. */
  pending?: boolean
  failed?: boolean
}

/** A Block Kit block or element. Shapes vary by `type`, so fields are loose. */
export type Block = { type: string; block_id?: string; [key: string]: any }

/** Legacy message attachment. */
export type Attachment = {
  color?: string
  fallback?: string
  pretext?: string
  author_name?: string
  author_link?: string
  author_icon?: string
  title?: string
  title_link?: string
  text?: string
  fields?: { title?: string; value?: string; short?: boolean }[]
  image_url?: string
  thumb_url?: string
  footer?: string
  footer_icon?: string
  ts?: number | string
  blocks?: Block[]
  mrkdwn_in?: string[]
}

/** What the user has entered in a surface: block_id → action_id → value. */
export type StateValues = Record<string, Record<string, { type: string; [key: string]: any }>>

/** A modal or App Home view. */
export type View = {
  id: string
  type: 'modal' | 'home'
  title?: Block
  submit?: Block | null
  close?: Block | null
  submit_disabled?: boolean
  blocks: Block[]
  state: { values: StateValues }
  hash: string
  app_id: string
  callback_id: string
  root_view_id: string
  previous_view_id?: string | null
}

export type SlashCommand = {
  command: string
  description: string
  usage_hint?: string | null
  app_id: string
  app_name: string
}

export type AppHomeInfo = {
  id: string
  name: string
  description?: string | null
  bot_user_id: string
  home_tab_enabled: boolean
  messages_tab_enabled: boolean
  messages_tab_read_only: boolean
  interactivity: boolean
  commands: SlashCommand[]
}

export type Counts = {
  id: string
  last_read: string
  latest?: string | null
  unread_count: number
  mention_count: number
}

export type Page<T> = T & { response_metadata?: { next_cursor?: string } }

export type Me = {
  account: { id: string; email: string; display_name: string }
  workspaces: Membership[]
}

export class ApiError extends Error {
  constructor(
    public code: string,
    public status: number,
    /** The whole error response, e.g. `errors` for `invalid_manifest`. */
    public data: Record<string, any> = {},
  ) {
    super(code)
  }
}

async function request<T>(method: 'GET' | 'POST', path: string, body?: unknown, headers: Record<string, string> = {}): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: 'same-origin',
    headers: body === undefined ? headers : { 'Content-Type': 'application/json', ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const data = await res.json().catch(() => ({ ok: false, error: `http_${res.status}` }))
  if (!res.ok || data.ok === false) throw new ApiError(data.error ?? `http_${res.status}`, res.status, data)
  return data as T
}

export const auth = {
  me: () => request<Me>('GET', '/auth/me'),
  login: (email: string, password: string) => request<Me>('POST', '/auth/login', { email, password }),
  signup: (email: string, password: string, display_name: string) =>
    request<Me>('POST', '/auth/signup', { email, password, display_name }),
  logout: () => request<unknown>('POST', '/auth/logout', {}),
  workspaces: () => request<{ workspaces: Team[] }>('GET', '/auth/workspaces'),
  createWorkspace: (name: string, domain: string) =>
    request<Membership>('POST', '/auth/workspaces', { name, domain }),
  joinWorkspace: (team_id: string) => request<Membership>('POST', '/auth/workspaces/join', { team_id }),
}

/** Calls a Slack Web API method as the signed-in user in the given workspace. */
export function slack<T = Record<string, unknown>>(teamId: string, method: string, args: Record<string, unknown> = {}) {
  return request<T & { ok: true }>('POST', `/api/${method}`, args, { 'X-Slack-Emu-Team': teamId })
}

/** Follows `next_cursor` until exhausted, concatenating `key` from each page. */
export async function slackAll<T>(teamId: string, method: string, key: string, args: Record<string, unknown> = {}): Promise<T[]> {
  const out: T[] = []
  let cursor: string | undefined
  do {
    const page = await slack<Page<Record<string, T[]>>>(teamId, method, { ...args, limit: 1000, cursor })
    out.push(...(page[key] ?? []))
    cursor = page.response_metadata?.next_cursor || undefined
  } while (cursor)
  return out
}

const messages: Record<string, string> = {
  invalid_credentials: 'That email and password don’t match.',
  email_taken: 'An account with that email already exists.',
  password_too_short: 'Passwords need at least 8 characters.',
  invalid_email: 'That doesn’t look like an email address.',
  signup_disabled: 'Sign-ups are turned off on this server.',
  domain_taken: 'That workspace URL is already taken.',
  invalid_domain: 'Use lowercase letters, numbers and dashes (max 21).',
  invalid_name: 'Give the workspace a name.',
}

export function describeError(e: unknown): string {
  if (e instanceof ApiError) return messages[e.code] ?? `Something went wrong (${e.code}).`
  return 'Couldn’t reach the server.'
}
