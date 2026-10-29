import { useEffect, useMemo, useState } from 'react'
import type { Me, Membership } from './api'
import { WorkspaceContext, type DialogKind, type WorkspaceCtx } from './context'
import { useRealtime } from './realtime'
import { clientPath, navigate, parseClientPath, usePath } from './router'
import { WorkspaceStore, convName, useWorkspace } from './store'
import { Sidebar } from './Sidebar'
import { ChannelView } from './ChannelView'
import { ThreadPane } from './ThreadPane'
import { Dialogs } from './Dialogs'
import { AppsPage } from './AppsPage'
import { InspectorPage } from './InspectorPage'
import { Modals } from './ViewModal'
import { WorkspaceDialog, initials } from './WorkspaceDialog'

const MOBILE = '(max-width: 767px)'

function useMediaQuery(q: string) {
  const [matches, setMatches] = useState(() => matchMedia(q).matches)
  useEffect(() => {
    const mql = matchMedia(q)
    const onChange = () => setMatches(mql.matches)
    mql.addEventListener('change', onChange)
    return () => mql.removeEventListener('change', onChange)
  }, [q])
  return matches
}

function lastChannelKey(team: string) {
  return `slack-emu:last-channel:${team}`
}

/** The signed-in app shell: workspace rail, sidebar, conversation and thread panes. */
export function Client({
  me,
  membership,
  onWorkspacesChanged,
  onSignedOut,
}: {
  me: Me
  membership: Membership
  onWorkspacesChanged: () => Promise<void>
  onSignedOut: () => void
}) {
  const { team, user } = membership
  // A fresh store per workspace; the key on <WorkspaceSession> resets everything else.
  const store = useMemo(() => new WorkspaceStore(team.id, user.id), [team.id, user.id])
  return (
    <WorkspaceSession
      key={team.id}
      store={store}
      me={me}
      membership={membership}
      onWorkspacesChanged={onWorkspacesChanged}
      onSignedOut={onSignedOut}
    />
  )
}

function WorkspaceSession({
  store,
  me,
  membership,
  onWorkspacesChanged,
  onSignedOut,
}: {
  store: WorkspaceStore
  me: Me
  membership: Membership
  onWorkspacesChanged: () => Promise<void>
  onSignedOut: () => void
}) {
  const { team } = membership
  const path = usePath()
  const { channel, thread, apps, app, inspector } = parseClientPath(path)
  const isMobile = useMediaQuery(MOBILE)
  const { status, sendTyping, typingDone } = useRealtime(store)
  const ready = useWorkspace(store, (s) => s.ready)
  const convs = useWorkspace(store, (s) => s.convs)
  const counts = useWorkspace(store, (s) => s.counts)
  const users = useWorkspace(store, (s) => s.users)
  const [dialog, setDialog] = useState<DialogKind | null>(null)
  const [addingWorkspace, setAddingWorkspace] = useState(false)

  const open = (ch?: string, th?: string) => navigate(clientPath(team.id, ch, th))

  // Desktop always shows a conversation: restore the last one, else #general.
  useEffect(() => {
    if (!ready || channel || apps || inspector || isMobile) return
    let last: string | null = null
    try {
      last = localStorage.getItem(lastChannelKey(team.id))
    } catch {
      // Storage may be unavailable; fall through to #general.
    }
    const target =
      (last && convs[last]?.is_member && last) || Object.values(convs).find((c) => c.is_general)?.id
    if (target) navigate(clientPath(team.id, target), true)
  }, [ready, channel, isMobile, convs, team.id])

  useEffect(() => {
    if (!channel) return
    try {
      localStorage.setItem(lastChannelKey(team.id), channel)
    } catch {
      // Best effort.
    }
  }, [channel, team.id])

  // Window title with unread total.
  useEffect(() => {
    const unread = Object.values(counts).filter((c) => convs[c.id]?.is_member && (c.mention_count > 0 || (convs[c.id]?.is_im && c.unread_count > 0))).length
    const where = channel && convs[channel] ? `${convs[channel].is_im || convs[channel].is_mpim ? '' : '#'}${convName(convs[channel], { users, me: store.state.me })} - ` : ''
    document.title = `${unread ? `(${unread}) ` : ''}${where}${team.name} - slack-emu`
  }, [counts, convs, users, channel, team.name, store])

  const ctx: WorkspaceCtx = { store, membership, sendTyping, typingDone, open, showDialog: setDialog }
  const mobileView = thread ? 'thread' : channel || apps || inspector ? 'main' : 'nav'

  return (
    <WorkspaceContext.Provider value={ctx}>
      <div className={`client view-${mobileView} ${thread ? 'has-thread' : ''}`}>
        <nav className="rail" aria-label="Workspaces">
          {me.workspaces.map((w) => (
            <button
              key={w.team.id}
              className={`team-avatar ${w.team.id === team.id ? 'active' : ''}`}
              title={w.team.name}
              aria-current={w.team.id === team.id ? 'page' : undefined}
              onClick={() => navigate(`/client/${w.team.id}`)}
            >
              {initials(w.team.name)}
            </button>
          ))}
          <button className="team-avatar add" title="Add a workspace" onClick={() => setAddingWorkspace(true)}>
            +
          </button>
        </nav>

        <Sidebar
          me={me}
          current={channel}
          page={apps ? 'apps' : inspector ? 'inspector' : undefined}
          status={status}
          onSignedOut={onSignedOut}
        />

        <main className="main">
          {!ready ? (
            <div className="pane-loading">Loading…</div>
          ) : apps ? (
            <AppsPage key={app ?? 'list'} app={app} />
          ) : inspector ? (
            <InspectorPage />
          ) : channel ? (
            <ChannelView key={channel} channel={channel} />
          ) : (
            <div className="pane-loading">Pick a conversation</div>
          )}
        </main>

        {ready && channel && thread && <ThreadPane key={`${channel}/${thread}`} channel={channel} thread={thread} />}
      </div>

      {dialog && <Dialogs dialog={dialog} onClose={() => setDialog(null)} />}
      {ready && <Modals />}
      {addingWorkspace && (
        <div className="overlay" onClick={(e) => e.target === e.currentTarget && setAddingWorkspace(false)}>
          <WorkspaceDialog
            onCancel={() => setAddingWorkspace(false)}
            onDone={async (m) => {
              setAddingWorkspace(false)
              await onWorkspacesChanged()
              navigate(`/client/${m.team.id}`)
            }}
          />
        </div>
      )}
    </WorkspaceContext.Provider>
  )
}
