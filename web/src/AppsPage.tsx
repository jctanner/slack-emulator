import { Fragment, useCallback, useEffect, useState } from 'react'
import { dump, load } from 'js-yaml'
import { useWs } from './context'
import { clientPath, navigate } from './router'
import { useWorkspace } from './store'
import { displayName } from './mrkdwn'

type AppSummary = {
  id: string
  name: string
  description?: string
  creator?: string
  installed: boolean
  needs_reinstall: boolean
  bot_user_id?: string
  bot_id?: string
  socket_mode: boolean
  socket_connections: number
  request_url?: string
  request_url_verified: boolean
  bot_events: string[]
  bot_scopes: string[]
  interactivity: boolean
  interactivity_url?: string | null
  slash_commands: { command: string; description: string; url?: string | null }[]
  home_tab_enabled: boolean
}

type AppInfo = AppSummary & {
  credentials: { client_id: string; client_secret: string; signing_secret: string; verification_token: string }
  manifest: unknown
  tokens: { token: string; type: 'bot' | 'user' | 'app'; user_id?: string; scopes: string[]; name?: string; created: number }[]
}

type Delivery = {
  time: number
  label: string
  transport: 'http' | 'socket'
  attempt: number
  ok: boolean
  error?: string
  latency_ms: number
  payload: unknown
  response?: unknown
}

type ManifestError = { message: string; pointer: string }

const TEMPLATE = `display_information:
  name: My Bot
  description: Says hello
features:
  bot_user:
    display_name: My Bot
    always_online: true
  app_home:
    home_tab_enabled: true
    messages_tab_enabled: true
  slash_commands:
    - command: /hello
      description: Say hello
      usage_hint: "[name]"
      # url: https://your-bot.example/slack/events   # for HTTP delivery
oauth_config:
  scopes:
    bot:
      - app_mentions:read
      - channels:history
      - channels:read
      - chat:write
      - commands
      - im:history
      - reactions:read
      - users:read
settings:
  event_subscriptions:
    # request_url: https://your-bot.example/slack/events   # for HTTP delivery
    bot_events:
      - app_home_opened
      - app_mention
      - message.channels
      - message.im
  interactivity:
    is_enabled: true
    # request_url: https://your-bot.example/slack/events   # for HTTP delivery
  socket_mode_enabled: true
`

function useCall() {
  const { store } = useWs()
  return store.call.bind(store)
}

/** `/client/T/apps` and `/client/T/apps/A123`. */
export function AppsPage({ app }: { app?: string }) {
  const { membership } = useWs()
  return (
    <>
      <header className="main-head">
        <button
          className="back icon-button"
          onClick={() => navigate(app ? clientPath(membership.team.id) + '/apps' : clientPath(membership.team.id))}
          aria-label="Back"
        >
          ‹
        </button>
        <h1>{app ? 'App settings' : 'Apps'}</h1>
      </header>
      <div className="page-body">{app ? <AppDetail id={app} /> : <AppList />}</div>
    </>
  )
}

function AppList() {
  const { membership } = useWs()
  const call = useCall()
  const [apps, setApps] = useState<AppSummary[]>()
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string>()

  useEffect(() => {
    call<{ apps: AppSummary[] }>('emu.apps.list').then((r) => setApps(r.apps), (e) => setError(e.code))
  }, [])

  return (
    <div className="apps-page">
      <div className="page-intro">
        <p className="muted">
          Apps are defined by Slack app manifests. Install one to get a bot token, then point your Bolt app at{' '}
          <code>{location.origin}/api/</code>.
        </p>
        <button className="primary-small" onClick={() => setCreating(true)}>
          Create an app
        </button>
      </div>
      {error && <p className="error">Couldn’t load apps: {error}</p>}
      {apps === undefined ? (
        <p className="muted">Loading…</p>
      ) : apps.length === 0 ? (
        <p className="empty-state">No apps yet.</p>
      ) : (
        <ul className="app-cards">
          {apps.map((a) => (
            <li key={a.id}>
              <button className="app-card" onClick={() => navigate(`${clientPath(membership.team.id)}/apps/${a.id}`)}>
                <span className="app-icon" aria-hidden>
                  {a.name.slice(0, 1).toUpperCase()}
                </span>
                <span className="app-card-main">
                  <strong>{a.name}</strong>
                  <span className="muted small">{a.description || a.id}</span>
                </span>
                <span className="app-card-status">
                  <StatusPill ok={a.installed} label={a.installed ? 'Installed' : 'Not installed'} />
                  <Transport app={a} />
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {creating && (
        <ManifestDialog
          title="Create an app from a manifest"
          initial={TEMPLATE}
          submitLabel="Create"
          onClose={() => setCreating(false)}
          onSubmit={async (manifest) => {
            const r = await call<{ app_id: string }>('apps.manifest.create', { manifest })
            navigate(`${clientPath(membership.team.id)}/apps/${r.app_id}`)
          }}
        />
      )}
    </div>
  )
}

function StatusPill({ ok, label }: { ok: boolean; label: string }) {
  return <span className={`pill ${ok ? 'pill-ok' : 'pill-off'}`}>{label}</span>
}

function Transport({ app }: { app: AppSummary }) {
  if (app.socket_mode) {
    return (
      <span className="muted small">
        Socket Mode · {app.socket_connections} {app.socket_connections === 1 ? 'connection' : 'connections'}
      </span>
    )
  }
  if (app.request_url) {
    return <span className="muted small">HTTP · {app.request_url_verified ? 'URL verified' : 'URL not verified'}</span>
  }
  return <span className="muted small">No event delivery</span>
}

function Secret({ value, label }: { value: string; label: string }) {
  const [shown, setShown] = useState(false)
  const [copied, setCopied] = useState(false)
  return (
    <div className="secret">
      <span className="secret-label">{label}</span>
      <code className="secret-value">{shown ? value : '•'.repeat(Math.min(value.length, 24))}</code>
      <button onClick={() => setShown((s) => !s)}>{shown ? 'Hide' : 'Show'}</button>
      <button
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(value)
            setCopied(true)
            setTimeout(() => setCopied(false), 1500)
          } catch {
            setShown(true)
          }
        }}
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
    </div>
  )
}

function AppDetail({ id }: { id: string }) {
  const { store, membership } = useWs()
  const call = useCall()
  const users = useWorkspace(store, (s) => s.users)
  const [app, setApp] = useState<AppInfo>()
  const [error, setError] = useState<string>()
  const [tab, setTab] = useState<'overview' | 'manifest' | 'events'>('overview')
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string>()

  const refresh = useCallback(
    () => call<{ app: AppInfo }>('emu.apps.info', { app_id: id }).then((r) => setApp(r.app), (e) => setError(e.code)),
    [id],
  )
  useEffect(() => {
    refresh()
    // Socket connection counts change as bots come and go.
    const t = setInterval(refresh, 3000)
    return () => clearInterval(t)
  }, [refresh])

  if (error) return <p className="error">{error === 'invalid_app_id' ? 'That app doesn’t exist.' : error === 'not_allowed' ? 'Only the app’s creator or a workspace admin can manage it.' : `Couldn’t load: ${error}`}</p>
  if (!app) return <p className="muted">Loading…</p>

  const act = async (method: string, args: Record<string, unknown> = {}, done?: (r: any) => void) => {
    setBusy(true)
    setNote(undefined)
    try {
      const r = await call(method, { app_id: id, ...args })
      done?.(r)
      await refresh()
    } catch (e: any) {
      setNote(`${method} failed: ${e.code ?? e}`)
    } finally {
      setBusy(false)
    }
  }

  const openApp = async (botUser: string) => {
    try {
      const r = await call<{ channel: { id: string } }>('conversations.open', { users: botUser })
      navigate(clientPath(membership.team.id, r.channel.id))
    } catch (e: any) {
      setNote(`Couldn’t open the app: ${e.code ?? e}`)
    }
  }

  const bot = app.tokens.find((t) => t.type === 'bot')
  const userTokens = app.tokens.filter((t) => t.type === 'user')
  const appTokens = app.tokens.filter((t) => t.type === 'app')

  return (
    <div className="apps-page">
      <div className="app-title">
        <span className="app-icon large" aria-hidden>
          {app.name.slice(0, 1).toUpperCase()}
        </span>
        <div>
          <h2>{app.name}</h2>
          <p className="muted small">
            {app.id}
            {app.creator && users[app.creator] && ` · created by ${displayName(users[app.creator])}`}
          </p>
        </div>
      </div>

      <div className="tabs" role="tablist">
        {(['overview', 'manifest', 'events'] as const).map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? 'active' : ''} onClick={() => setTab(t)}>
            {t === 'events' ? 'Event log' : t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>
      {note && <p className="error">{note}</p>}

      {tab === 'overview' && (
        <>
          <section className="panel">
            <h3>Installation</h3>
            {app.installed ? (
              <>
                <p>
                  <StatusPill ok label="Installed" /> as{' '}
                  {app.bot_user_id && users[app.bot_user_id] ? <strong>@{users[app.bot_user_id].name}</strong> : 'a bot'}
                </p>
                {app.needs_reinstall && <p className="warn">Scopes changed since the last install. Reinstall to apply them.</p>}
                {bot && <Secret label="Bot token (xoxb)" value={bot.token} />}
                {app.bot_user_id && (
                  <p>
                    <button className="primary-small" onClick={() => openApp(app.bot_user_id!)}>
                      Open app
                    </button>{' '}
                    <span className="muted small">Its Home tab and DM{app.home_tab_enabled ? '' : ' (Home tab is off)'}</span>
                  </p>
                )}
                {userTokens.map((t) => (
                  <Secret key={t.token} label={`User token for ${displayName(users[t.user_id ?? ''], t.user_id)}`} value={t.token} />
                ))}
                <div className="row-actions left">
                  <button disabled={busy} onClick={() => act('emu.apps.install')}>
                    Reinstall
                  </button>
                  <button
                    disabled={busy}
                    className="danger"
                    onClick={() => confirm('Uninstall? Bot and user tokens will be revoked.') && act('emu.apps.uninstall')}
                  >
                    Uninstall
                  </button>
                </div>
              </>
            ) : (
              <>
                <p className="muted">Install the app to create its bot user and get tokens.</p>
                <button className="primary-small" disabled={busy} onClick={() => act('emu.apps.install')}>
                  Install to {membership.team.name}
                </button>
              </>
            )}
          </section>

          <section className="panel">
            <h3>Event delivery</h3>
            {app.socket_mode ? (
              <>
                <p>
                  <StatusPill ok={app.socket_connections > 0} label={app.socket_connections > 0 ? 'Connected' : 'Not connected'} />{' '}
                  Socket Mode · {app.socket_connections} open {app.socket_connections === 1 ? 'connection' : 'connections'}
                </p>
                {appTokens.length === 0 && <p className="muted small">Socket Mode needs an app-level token with connections:write.</p>}
              </>
            ) : app.request_url ? (
              <>
                <p>
                  <StatusPill ok={app.request_url_verified} label={app.request_url_verified ? 'Verified' : 'Not verified'} /> <code>{app.request_url}</code>
                </p>
                {!app.request_url_verified && (
                  <>
                    <p className="muted small">
                      Events are only sent once the URL answers Slack’s <code>url_verification</code> challenge. Start your app, then retry.
                    </p>
                    <button
                      disabled={busy}
                      onClick={() =>
                        act('emu.apps.verifyUrl', {}, (r) => setNote(r.verified ? undefined : `Verification failed: ${r.message}`))
                      }
                    >
                      Retry verification
                    </button>
                  </>
                )}
              </>
            ) : (
              <p className="muted">No request URL and Socket Mode is off, so no events are delivered.</p>
            )}
            <p className="muted small">
              Subscribed: {app.bot_events.length ? app.bot_events.map((e) => <code key={e}>{e}</code>).reduce((a, b) => <>{a}, {b}</>) : 'nothing'}
            </p>
          </section>

          <section className="panel">
            <h3>Interactivity and commands</h3>
            <p>
              <StatusPill ok={app.interactivity} label={app.interactivity ? 'Interactivity on' : 'Interactivity off'} />{' '}
              {app.interactivity && (app.socket_mode ? 'over Socket Mode' : <code>{app.interactivity_url}</code>)}
            </p>
            {!app.interactivity && (
              <p className="muted small">
                Buttons, menus and modals need <code>settings.interactivity.is_enabled: true</code> in the manifest.
              </p>
            )}
            {app.slash_commands.length > 0 ? (
              <dl className="kv">
                {app.slash_commands.map((c) => (
                  <Fragment key={c.command}>
                    <dt>
                      <code>{c.command}</code>
                    </dt>
                    <dd>
                      {c.description}
                      {!app.socket_mode && c.url && <span className="muted small"> → {c.url}</span>}
                    </dd>
                  </Fragment>
                ))}
              </dl>
            ) : (
              <p className="muted small">No slash commands.</p>
            )}
          </section>

          <section className="panel">
            <h3>App-level tokens</h3>
            {appTokens.map((t) => (
              <div key={t.token} className="token-row">
                <Secret label={`${t.name ?? 'token'} (${t.scopes.join(', ')})`} value={t.token} />
                <button disabled={busy} className="danger" onClick={() => confirm('Revoke this token?') && act('emu.apps.tokens.revoke', { target: t.token })}>
                  Revoke
                </button>
              </div>
            ))}
            <button disabled={busy} onClick={() => act('emu.apps.tokens.create', { name: `token-${appTokens.length + 1}`, scopes: 'connections:write' })}>
              Generate token (connections:write)
            </button>
          </section>

          <section className="panel">
            <h3>Credentials</h3>
            <Secret label="Client ID" value={app.credentials.client_id} />
            <Secret label="Client secret" value={app.credentials.client_secret} />
            <Secret label="Signing secret" value={app.credentials.signing_secret} />
            <Secret label="Verification token" value={app.credentials.verification_token} />
          </section>

          <section className="panel danger-panel">
            <button
              className="danger"
              disabled={busy}
              onClick={async () => {
                if (!confirm(`Delete ${app.name}? This can’t be undone.`)) return
                await act('apps.manifest.delete', {}, () => navigate(`${clientPath(membership.team.id)}/apps`))
              }}
            >
              Delete app
            </button>
          </section>
        </>
      )}

      {tab === 'manifest' && (
        <section className="panel">
          <pre className="manifest-view">{dump(app.manifest, { lineWidth: 120 })}</pre>
          <button className="primary-small" onClick={() => setEditing(true)}>
            Edit manifest
          </button>
        </section>
      )}

      {tab === 'events' && <EventLog appId={id} />}

      {editing && (
        <ManifestDialog
          title="Edit manifest"
          initial={dump(app.manifest, { lineWidth: 120 })}
          submitLabel="Save"
          onClose={() => setEditing(false)}
          onSubmit={async (manifest) => {
            const r = await call<{ permissions_updated: boolean; response_metadata?: { messages?: string[] } }>('apps.manifest.update', {
              app_id: id,
              manifest,
            })
            const msgs: string[] = []
            if (r.permissions_updated && app.installed) msgs.push('Scopes changed: reinstall to apply them.')
            if (r.response_metadata?.messages?.length) msgs.push(`Request URL not verified: ${r.response_metadata.messages.join(' ')}`)
            setNote(msgs.join(' ') || undefined)
            setEditing(false)
            await refresh()
          }}
        />
      )}
    </div>
  )
}

function ManifestDialog({
  title,
  initial,
  submitLabel,
  onClose,
  onSubmit,
}: {
  title: string
  initial: string
  submitLabel: string
  onClose: () => void
  onSubmit: (manifest: unknown) => Promise<void>
}) {
  const [text, setText] = useState(initial)
  const [errors, setErrors] = useState<ManifestError[]>([])
  const [parseError, setParseError] = useState<string>()
  const [busy, setBusy] = useState(false)

  const submit = async () => {
    setParseError(undefined)
    setErrors([])
    let manifest: unknown
    try {
      // YAML is a superset of JSON, so this accepts either.
      manifest = load(text)
    } catch (e: any) {
      setParseError(e.message)
      return
    }
    setBusy(true)
    try {
      await onSubmit(manifest)
    } catch (e: any) {
      if (e.code === 'invalid_manifest' && e.data?.errors) setErrors(e.data.errors)
      else setParseError(e.code ?? String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="card modal wide" role="dialog" aria-modal="true">
        <div className="dialog-head">
          <h1>{title}</h1>
          <button className="icon-button" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        <p className="muted small">YAML or JSON, in Slack’s app manifest format.</p>
        <textarea className="manifest-editor" spellCheck={false} value={text} onChange={(e) => setText(e.target.value)} />
        {parseError && <p className="error">{parseError}</p>}
        {errors.length > 0 && (
          <ul className="manifest-errors">
            {errors.map((e, i) => (
              <li key={i}>
                <code>{e.pointer}</code> {e.message}
              </li>
            ))}
          </ul>
        )}
        <button className="primary" disabled={busy} onClick={submit}>
          {submitLabel}
        </button>
      </div>
    </div>
  )
}

function EventLog({ appId }: { appId: string }) {
  const call = useCall()
  const [rows, setRows] = useState<Delivery[]>([])
  const [open, setOpen] = useState<number | null>(null)

  useEffect(() => {
    const load = () => call<{ deliveries: Delivery[] }>('emu.apps.deliveries', { app_id: appId }).then((r) => setRows(r.deliveries), () => {})
    load()
    const t = setInterval(load, 2000)
    return () => clearInterval(t)
  }, [appId])

  if (rows.length === 0) return <p className="empty-state">Nothing delivered yet. Events appear here as they’re sent.</p>
  return (
    <ul className="event-log">
      {rows.map((d, i) => (
        <li key={`${d.time}-${i}`} className={d.ok ? '' : 'failed'}>
          <button className="event-row" onClick={() => setOpen(open === i ? null : i)}>
            <span className={`dot ${d.ok ? 'ok' : 'bad'}`} aria-hidden />
            <span className="event-label">{d.label}</span>
            <span className="muted small">
              {d.transport}
              {d.attempt > 0 && ` · retry ${d.attempt}`}
            </span>
            <span className="muted small event-result">{d.ok ? `${d.latency_ms} ms` : d.error}</span>
            <time className="muted small">{new Date(d.time * 1000).toLocaleTimeString()}</time>
          </button>
          {open === i && (
            <pre className="event-payload">
              {JSON.stringify(d.payload, null, 2)}
              {d.response !== undefined && d.response !== null && `\n\n// response\n${JSON.stringify(d.response, null, 2)}`}
            </pre>
          )}
        </li>
      ))}
    </ul>
  )
}
