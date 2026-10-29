import { useMemo, useState } from 'react'
import { auth, type Conversation, type Me } from './api'
import { useWs } from './context'
import type { ConnectionStatus } from './realtime'
import { clientPath, navigate } from './router'
import { convName, useWorkspace } from './store'
import { Avatar } from './Avatar'
import { initials } from './WorkspaceDialog'

export function Sidebar({
  me,
  current,
  page,
  status,
  onSignedOut,
}: {
  me: Me
  current?: string
  /** A page other than a conversation is showing. */
  page?: 'apps' | 'inspector'
  status: ConnectionStatus
  onSignedOut: () => void
}) {
  const { store, membership, open, showDialog } = useWs()
  const { team, user } = membership
  const convs = useWorkspace(store, (s) => s.convs)
  const counts = useWorkspace(store, (s) => s.counts)
  const users = useWorkspace(store, (s) => s.users)
  const [menuOpen, setMenuOpen] = useState(false)

  // DMs with bots are listed under Apps, as in Slack.
  const isBotDm = (c: Conversation) => !!c.is_im && !!users[c.user ?? '']?.is_bot
  const { channels, dms, appDms } = useMemo(() => {
    const mine = Object.values(convs).filter((c) => c.is_member && (!c.is_archived || c.id === current))
    const channels = mine.filter((c) => !c.is_im && !c.is_mpim).sort((a, b) => (a.name ?? '').localeCompare(b.name ?? ''))
    const latest = (c: Conversation) => counts[c.id]?.latest ?? String(c.created)
    const dms = mine
      .filter((c) => (c.is_im || c.is_mpim) && !isBotDm(c))
      .sort((a, b) => (latest(b) > latest(a) ? 1 : -1))
    const appDms = mine.filter(isBotDm).sort((a, b) => convName(a, { users, me: store.state.me }).localeCompare(convName(b, { users, me: store.state.me })))
    return { channels, dms, appDms }
  }, [convs, counts, current, users])

  const row = (c: Conversation) => {
    const n = counts[c.id]
    const unread = !!n && n.unread_count > 0 && c.id !== current
    // In DMs every unread message counts as a mention.
    const badge = c.id === current ? 0 : c.is_im || c.is_mpim ? (n?.unread_count ?? 0) : (n?.mention_count ?? 0)
    const name = convName(c, { users, me: store.state.me })
    return (
      <button
        key={c.id}
        className={`sidebar-item ${c.id === current ? 'active' : ''} ${unread ? 'unread' : ''}`}
        onClick={() => open(c.id)}
        aria-current={c.id === current ? 'page' : undefined}
      >
        {c.is_im ? (
          <Avatar user={users[c.user ?? '']} id={c.user ?? c.id} size={20} />
        ) : c.is_mpim ? (
          <span className="sidebar-icon mpim-count" aria-hidden>
            {name.split(', ').length}
          </span>
        ) : (
          <span className="sidebar-icon" aria-hidden>
            {c.is_private ? '🔒︎' : '#'}
          </span>
        )}
        <span className="sidebar-label">{name}</span>
        {badge > 0 && <span className="badge">{badge}</span>}
      </button>
    )
  }

  return (
    <aside className="sidebar">
      <header className="sidebar-head">
        <span className="sidebar-team">{team.name}</span>
        <div className="menu-anchor">
          <button
            className="user-chip"
            onClick={() => setMenuOpen((o) => !o)}
            aria-expanded={menuOpen}
            title="Account"
          >
            {initials(user.real_name || user.name)}
          </button>
          {menuOpen && (
            <div className="menu" role="menu" onClick={() => setMenuOpen(false)}>
              <div className="menu-who">
                <strong>{user.real_name || user.name}</strong>
                <span className="muted">@{user.name}</span>
                <span className="muted">{me.account.email}</span>
              </div>
              <button role="menuitem" onClick={() => showDialog({ kind: 'connection' })}>
                Connection details
              </button>
              <button
                role="menuitem"
                onClick={async () => {
                  await auth.logout().catch(() => {})
                  onSignedOut()
                }}
              >
                Sign out of slack-emu
              </button>
            </div>
          )}
        </div>
      </header>
      {status === 'reconnecting' && <div className="conn-banner">Reconnecting…</div>}
      <div className="sidebar-body">
        <section className="sidebar-section">
          <h2>Channels</h2>
          {channels.map(row)}
          <button className="sidebar-item add" onClick={() => showDialog({ kind: 'browse-channels' })}>
            <span className="sidebar-icon" aria-hidden>
              +
            </span>
            <span className="sidebar-label">Browse channels</span>
          </button>
          <button className="sidebar-item add" onClick={() => showDialog({ kind: 'create-channel' })}>
            <span className="sidebar-icon" aria-hidden>
              +
            </span>
            <span className="sidebar-label">Create a channel</span>
          </button>
        </section>
        <section className="sidebar-section">
          <h2>Direct messages</h2>
          {dms.map(row)}
          <button className="sidebar-item add" onClick={() => showDialog({ kind: 'new-message' })}>
            <span className="sidebar-icon" aria-hidden>
              +
            </span>
            <span className="sidebar-label">New message</span>
          </button>
        </section>
        <section className="sidebar-section">
          <h2>Apps</h2>
          {appDms.map(row)}
          <button
            className={`sidebar-item add ${page === 'apps' ? 'active' : ''}`}
            onClick={() => navigate(`${clientPath(team.id)}/apps`)}
          >
            <span className="sidebar-icon" aria-hidden>
              ⚙︎
            </span>
            <span className="sidebar-label">Manage apps</span>
          </button>
          {(users[user.id]?.is_admin || users[user.id]?.is_owner) && (
            <button
              className={`sidebar-item add ${page === 'inspector' ? 'active' : ''}`}
              onClick={() => navigate(`${clientPath(team.id)}/inspector`)}
            >
              <span className="sidebar-icon" aria-hidden>
                ⌕
              </span>
              <span className="sidebar-label">Inspector</span>
            </button>
          )}
        </section>
      </div>
    </aside>
  )
}
