// An app's DM shows tabs like Slack's: Home (the app's `views.publish` view),
// Messages (the DM itself) and About.

import { Fragment, useEffect, useMemo, useState } from 'react'
import type { AppHomeInfo, View } from './api'
import { Blocks, useBlockSurface, type Container } from './BlockKit'
import { useWs } from './context'
import { Mrkdwn } from './mrkdwn'
import { useWorkspace } from './store'

export type AppTab = 'home' | 'messages' | 'about'

/** App details for a DM with a bot, or `null` if it isn't an installed app's DM. */
export function useAppDm(channel: string, isBotDm: boolean) {
  const { store } = useWs()
  const [app, setApp] = useState<AppHomeInfo | null>(null)
  useEffect(() => {
    setApp(null)
    if (!isBotDm) return
    store
      .call<{ app: AppHomeInfo; view: View | null }>('emu.apps.home', { channel })
      .then((r) => {
        setApp(r.app)
        if (r.view) store.setHomeView(r.app.id, r.view)
      })
      .catch(() => setApp(null))
  }, [channel, isBotDm, store])
  return app
}

/** Tells the app the tab was opened (`app_home_opened`) and refreshes the Home view. */
export function useTabOpened(channel: string, app: AppHomeInfo | null, tab: AppTab | null) {
  const { store } = useWs()
  useEffect(() => {
    if (!app || !tab || tab === 'about') return
    store
      .call<{ view: View | null }>('emu.apps.home', { channel, tab })
      .then((r) => r.view && store.setHomeView(app.id, r.view))
      .catch(() => {})
  }, [channel, app?.id, tab, store])
}

export function AppTabs({ app, tab, onTab }: { app: AppHomeInfo; tab: AppTab | null; onTab: (t: AppTab) => void }) {
  const tabs: [AppTab, string][] = [
    ...(app.home_tab_enabled ? [['home', 'Home'] as [AppTab, string]] : []),
    ...(app.messages_tab_enabled ? [['messages', 'Messages'] as [AppTab, string]] : []),
    ['about', 'About'],
  ]
  return (
    <div className="tabs app-tabs" role="tablist">
      {tabs.map(([t, label]) => (
        <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? 'active' : ''} onClick={() => onTab(t)}>
          {label}
        </button>
      ))}
    </div>
  )
}

export function HomeTab({ app }: { app: AppHomeInfo }) {
  const { store } = useWs()
  const view = useWorkspace(store, (s) => s.homeViews[app.id])
  if (!view) {
    return (
      <div className="app-home empty">
        <p className="muted">{app.name} hasn’t published a Home tab for you yet.</p>
        <p className="muted small">
          Apps publish one with <code>views.publish</code>, usually when they receive <code>app_home_opened</code>.
        </p>
      </div>
    )
  }
  return <HomeView key={view.id} view={view} />
}

function HomeView({ view }: { view: View }) {
  const ws = useWs()
  const users = useWorkspace(ws.store, (s) => s.users)
  const convs = useWorkspace(ws.store, (s) => s.convs)
  const container = useMemo<Container>(() => ({ type: 'view', view_id: view.id }), [view.id])
  const surface = useBlockSurface(view.blocks, container, view.state?.values)
  const mctx = useMemo(() => ({ me: ws.store.state.me, users, convs, onChannel: (id: string) => ws.open(id) }), [ws, users, convs])
  return (
    <div className="app-home">
      <Blocks blocks={view.blocks} surface={{ ...surface, mctx }} />
    </div>
  )
}

export function AboutTab({ app }: { app: AppHomeInfo }) {
  const ws = useWs()
  const users = useWorkspace(ws.store, (s) => s.users)
  const convs = useWorkspace(ws.store, (s) => s.convs)
  const ctx = { me: ws.store.state.me, users, convs }
  return (
    <div className="app-home about">
      <h2>{app.name}</h2>
      {app.description && <Mrkdwn text={app.description} ctx={ctx} />}
      {app.commands.length > 0 && (
        <>
          <h3>Slash commands</h3>
          <dl className="kv">
            {app.commands.map((c) => (
              <Fragment key={c.command}>
                <dt>
                  <code>
                    {c.command}
                    {c.usage_hint ? ` ${c.usage_hint}` : ''}
                  </code>
                </dt>
                <dd>{c.description}</dd>
              </Fragment>
            ))}
          </dl>
        </>
      )}
      <p className="muted small">
        <button className="link" onClick={() => ws.open(undefined)}>
          Back to conversations
        </button>
        {' · '}
        <a href={`/client/${ws.membership.team.id}/apps/${app.id}`}>App settings</a>
      </p>
    </div>
  )
}
