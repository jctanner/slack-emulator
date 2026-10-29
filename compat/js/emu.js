// A tiny client for the emulator's browser-facing endpoints, used to set up
// users and apps the way a person would in the web UI.

import { randomUUID } from 'node:crypto'

export const EMU_URL = (process.env.EMU_URL ?? 'http://127.0.0.1:8080').replace(/\/$/, '')
export const API_URL = `${EMU_URL}/api/`

export class Person {
  static async signup(prefix) {
    const p = new Person()
    const email = `${prefix}-${randomUUID().slice(0, 8)}@example.com`
    const me = await p.post('/auth/signup', { email, password: 'password123', display_name: prefix })
    p.userId = me.workspaces[0].user.id
    return p
  }

  async post(path, body) {
    const res = await fetch(EMU_URL + path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(this.cookie ? { Cookie: this.cookie } : {}) },
      body: JSON.stringify(body),
    })
    const setCookie = res.headers.get('set-cookie')
    if (setCookie) this.cookie = setCookie.split(';')[0]
    return res.json()
  }

  async api(method, args = {}) {
    const r = await this.post(`/api/${method}`, args)
    if (!r.ok) throw new Error(`${method} failed: ${JSON.stringify(r)}`)
    return r
  }

  /** Fetches a file URL (url_private) with this person's session. */
  async download(url) {
    const res = await fetch(url, { headers: this.cookie ? { Cookie: this.cookie } : {} })
    if (!res.ok) throw new Error(`download failed: ${res.status}`)
    return Buffer.from(await res.arrayBuffer())
  }

  async channelId(name) {
    const r = await this.api('users.conversations', { types: 'public_channel,private_channel' })
    return r.channels.find((c) => c.name === name).id
  }

  /** Polls history (or a thread) until a message matches. */
  async waitForMessage(channel, predicate, { threadTs, timeout = 10_000 } = {}) {
    const deadline = Date.now() + timeout
    while (Date.now() < deadline) {
      const r = threadTs
        ? await this.api('conversations.replies', { channel, ts: threadTs })
        : await this.api('conversations.history', { channel, limit: 20 })
      const found = r.messages.find(predicate)
      if (found) return found
      await new Promise((ok) => setTimeout(ok, 100))
    }
    throw new Error(`no matching message in ${channel} within ${timeout}ms`)
  }
}

/** Polls `fn` until it returns something truthy, and returns that. */
export async function waitUntil(fn, what = 'condition', timeout = 10_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const got = await fn()
    if (got) return got
    await new Promise((ok) => setTimeout(ok, 100))
  }
  throw new Error(`timed out waiting for ${what}`)
}

/**
 * An app with events, interactivity, slash commands and a Home tab. HTTP apps
 * send everything to `requestUrl` (Bolt serves it all on one path).
 */
export function manifest(name, { socketMode, requestUrl, commands = [] } = {}) {
  const events = { bot_events: ['message.channels', 'app_mention', 'app_home_opened'] }
  const interactivity = { is_enabled: true }
  if (requestUrl) {
    events.request_url = requestUrl
    interactivity.request_url = requestUrl
  }
  return {
    display_information: { name },
    features: {
      bot_user: { display_name: name, always_online: true },
      app_home: { home_tab_enabled: true, messages_tab_enabled: true },
      slash_commands: commands.map((command) => ({
        command,
        description: `Test command ${command}`,
        ...(requestUrl ? { url: requestUrl } : {}),
      })),
    },
    oauth_config: {
      scopes: {
        bot: [
          'chat:write',
          'channels:history',
          'channels:read',
          'app_mentions:read',
          'users:read',
          'commands',
          'files:write',
          'files:read',
        ],
      },
    },
    settings: { event_subscriptions: events, interactivity, socket_mode_enabled: !!socketMode },
  }
}
