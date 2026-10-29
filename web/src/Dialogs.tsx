import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from 'react'
import { describeError, slackAll, type Conversation, type SlackUser } from './api'
import { useWs, type DialogKind } from './context'
import { displayName } from './mrkdwn'
import { convName, useWorkspace } from './store'
import { Avatar } from './Avatar'

export function Dialogs({ dialog, onClose }: { dialog: DialogKind; onClose: () => void }) {
  switch (dialog.kind) {
    case 'create-channel':
      return <CreateChannel onClose={onClose} />
    case 'browse-channels':
      return <BrowseChannels onClose={onClose} />
    case 'new-message':
      return <NewMessage onClose={onClose} />
    case 'channel-details':
      return <ChannelDetails channel={dialog.channel} initialTab={dialog.tab ?? 'about'} onClose={onClose} />
    case 'connection':
      return <ConnectionDetails onClose={onClose} />
  }
}

function Modal({ title, onClose, children, wide }: { title: ReactNode; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`card modal ${wide ? 'wide' : ''}`} role="dialog" aria-modal="true">
        <div className="dialog-head">
          <h1>{title}</h1>
          <button className="icon-button" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  )
}

function errorText(e: unknown): string {
  const code = (e as { code?: string })?.code
  const known: Record<string, string> = {
    name_taken: 'That name is already taken by a channel, username, or user group.',
    invalid_name_specials: 'Channel names can only contain lowercase letters, numbers, hyphens, and underscores.',
    invalid_name_required: 'Give the channel a name.',
    invalid_name_maxlength: 'Channel names must be 80 characters or fewer.',
    invalid_name: 'That name isn’t allowed.',
    cant_leave_general: 'Everyone stays in #general.',
    cant_archive_general: '#general can’t be archived.',
    cant_kick_from_general: 'Nobody can be removed from #general.',
    too_long: 'That’s too long (250 characters max).',
  }
  return (code && known[code]) || describeError(e)
}

/** Lowercases and replaces spaces as you type, like Slack. */
function channelSlug(s: string) {
  return s.toLowerCase().replace(/\s+/g, '-').slice(0, 80)
}

function CreateChannel({ onClose }: { onClose: () => void }) {
  const { store, open } = useWs()
  const [name, setName] = useState('')
  const [isPrivate, setPrivate] = useState(false)
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(undefined)
    try {
      const r = await store.call<{ channel: Conversation }>('conversations.create', { name, is_private: isPrivate })
      store.upsertConv({ ...r.channel, is_member: true })
      onClose()
      open(r.channel.id)
    } catch (err) {
      setError(errorText(err))
      setBusy(false)
    }
  }

  return (
    <Modal title="Create a channel" onClose={onClose}>
      <form onSubmit={submit}>
        <label>
          Name
          <div className="prefixed-input">
            <span aria-hidden>{isPrivate ? '🔒︎' : '#'}</span>
            <input
              autoFocus
              required
              value={name}
              maxLength={80}
              onChange={(e) => setName(channelSlug(e.target.value))}
              placeholder="e.g. plan-budget"
            />
          </div>
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={isPrivate} onChange={(e) => setPrivate(e.target.checked)} />
          <span>
            <strong>Make private</strong>
            <span className="muted small"> — only invited people can see and join it</span>
          </span>
        </label>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <button className="primary" disabled={busy || !name}>
          Create
        </button>
      </form>
    </Modal>
  )
}

function BrowseChannels({ onClose }: { onClose: () => void }) {
  const { store, open } = useWs()
  const [all, setAll] = useState<Conversation[]>()
  const [q, setQ] = useState('')
  const [showArchived, setShowArchived] = useState(false)
  const [error, setError] = useState<string>()

  useEffect(() => {
    slackAll<Conversation>(store.state.teamId, 'conversations.list', 'channels', {
      types: 'public_channel,private_channel',
    }).then(setAll, (e) => setError(errorText(e)))
  }, [store])

  const shown = (all ?? [])
    .filter((c) => showArchived || !c.is_archived)
    .filter((c) => !q || c.name?.includes(q.toLowerCase()) || c.purpose?.value.toLowerCase().includes(q.toLowerCase()))
    .sort((a, b) => (a.name ?? '').localeCompare(b.name ?? ''))

  const join = async (c: Conversation) => {
    try {
      const r = await store.call<{ channel: Conversation }>('conversations.join', { channel: c.id })
      store.upsertConv({ ...r.channel, is_member: true })
      onClose()
      open(c.id)
    } catch (e) {
      setError(errorText(e))
    }
  }

  return (
    <Modal title="All channels" onClose={onClose} wide>
      <input className="search" autoFocus placeholder="Search for channels" value={q} onChange={(e) => setQ(e.target.value)} />
      <label className="checkbox small">
        <input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} /> Show archived
        channels
      </label>
      {error && <p className="error">{error}</p>}
      {all === undefined ? (
        <p className="muted">Loading…</p>
      ) : (
        <ul className="pick-list">
          {shown.map((c) => (
            <li key={c.id}>
              <button
                className="pick-main"
                onClick={() => {
                  store.upsertConv(c)
                  onClose()
                  open(c.id)
                }}
              >
                <strong>
                  {c.is_private ? '🔒︎' : '#'} {c.name}
                </strong>
                <span className="muted small">
                  {c.is_member && <span className="joined">✓ Joined · </span>}
                  {c.num_members ?? 0} {c.num_members === 1 ? 'member' : 'members'}
                  {c.is_archived && ' · archived'}
                  {c.purpose?.value && ` · ${c.purpose.value}`}
                </span>
              </button>
              {!c.is_member && !c.is_archived && <button onClick={() => join(c)}>Join</button>}
            </li>
          ))}
          {shown.length === 0 && <li className="muted">No channels match.</li>}
        </ul>
      )}
    </Modal>
  )
}

function UserPicker({
  exclude,
  selected,
  onToggle,
}: {
  exclude: string[]
  selected: string[]
  onToggle: (u: SlackUser) => void
}) {
  const { store } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const [q, setQ] = useState('')
  const list = useMemo(
    () =>
      Object.values(users)
        .filter((u) => !u.deleted && !exclude.includes(u.id))
        .filter((u) => !q || [u.name, u.real_name, u.profile?.display_name].some((s) => s?.toLowerCase().includes(q.toLowerCase())))
        .sort((a, b) => displayName(a).localeCompare(displayName(b)))
        .slice(0, 50),
    [users, exclude, q],
  )
  return (
    <>
      <input className="search" autoFocus placeholder="Search by name" value={q} onChange={(e) => setQ(e.target.value)} />
      <ul className="pick-list">
        {list.map((u) => (
          <li key={u.id}>
            <label className="pick-main user-pick">
              <input type="checkbox" checked={selected.includes(u.id)} onChange={() => onToggle(u)} />
              <Avatar user={u} id={u.id} size={24} />
              <strong>{displayName(u)}</strong>
              <span className="muted small">@{u.name}</span>
              {u.is_bot && <span className="app-badge">APP</span>}
            </label>
          </li>
        ))}
        {list.length === 0 && <li className="muted">Nobody matches.</li>}
      </ul>
    </>
  )
}

function NewMessage({ onClose }: { onClose: () => void }) {
  const { store, open } = useWs()
  const [selected, setSelected] = useState<string[]>([])
  const [error, setError] = useState<string>()

  const start = async () => {
    try {
      const ids = selected.length ? selected : [store.state.me]
      const r = await store.call<{ channel: Conversation }>('conversations.open', { users: ids.join(',') })
      store.upsertConv({ ...r.channel, is_member: true })
      onClose()
      open(r.channel.id)
    } catch (e) {
      setError(errorText(e))
    }
  }

  return (
    <Modal title="New message" onClose={onClose}>
      <UserPicker
        exclude={[]}
        selected={selected}
        onToggle={(u) => setSelected((s) => (s.includes(u.id) ? s.filter((x) => x !== u.id) : [...s, u.id]))}
      />
      {error && <p className="error">{error}</p>}
      <button className="primary" onClick={start} disabled={selected.length > 8}>
        {selected.length > 1 ? `Start group conversation (${selected.length + 1})` : 'Start conversation'}
      </button>
    </Modal>
  )
}

function ChannelDetails({
  channel,
  initialTab,
  onClose,
}: {
  channel: string
  initialTab: 'about' | 'members' | 'settings'
  onClose: () => void
}) {
  const { store, open } = useWs()
  const conv = useWorkspace(store, (s) => s.convs[channel])
  const users = useWorkspace(store, (s) => s.users)
  const [tab, setTab] = useState(initialTab)
  const [members, setMembers] = useState<string[]>()
  const [adding, setAdding] = useState<string[] | null>(null)
  const [error, setError] = useState<string>()
  const [topic, setTopic] = useState(conv?.topic?.value ?? '')
  const [purpose, setPurpose] = useState(conv?.purpose?.value ?? '')
  const [name, setName] = useState(conv?.name ?? '')

  const loadMembers = () =>
    slackAll<string>(store.state.teamId, 'conversations.members', 'members', { channel }).then(setMembers, (e) =>
      setError(errorText(e)),
    )
  useEffect(() => {
    if (tab === 'members') loadMembers()
  }, [tab])

  if (!conv) return null
  const isDm = conv.is_im || conv.is_mpim
  const title = isDm ? convName(conv, { users, me: store.state.me }) : `${conv.is_private ? '🔒︎' : '#'} ${conv.name}`

  const act = async (method: string, args: Record<string, unknown>, after?: () => void) => {
    setError(undefined)
    try {
      await store.call(method, { channel, ...args })
      after?.()
    } catch (e) {
      setError(errorText(e))
    }
  }

  return (
    <Modal title={title} onClose={onClose} wide>
      <div className="tabs" role="tablist">
        {(['about', 'members', ...(isDm ? [] : ['settings'])] as const).map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? 'active' : ''} onClick={() => setTab(t as typeof tab)}>
            {t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>

      {tab === 'about' && (
        <div className="about">
          <label>
            Topic
            <div className="inline-edit">
              <input value={topic} maxLength={250} onChange={(e) => setTopic(e.target.value)} placeholder="Add a topic" disabled={!conv.is_member || conv.is_archived} />
              {topic !== (conv.topic?.value ?? '') && <button onClick={() => act('conversations.setTopic', { topic })}>Save</button>}
            </div>
          </label>
          {!isDm && (
            <label>
              Description
              <div className="inline-edit">
                <input value={purpose} maxLength={250} onChange={(e) => setPurpose(e.target.value)} placeholder="Add a description" disabled={!conv.is_member || conv.is_archived} />
                {purpose !== (conv.purpose?.value ?? '') && <button onClick={() => act('conversations.setPurpose', { purpose })}>Save</button>}
              </div>
            </label>
          )}
          <p className="muted small">
            {conv.creator && users[conv.creator] ? `Created by ${displayName(users[conv.creator])} on ` : 'Created '}
            {new Date(conv.created * 1000).toLocaleDateString()}
            {' · '}ID <code>{conv.id}</code>
          </p>
        </div>
      )}

      {tab === 'members' && (
        <div>
          {adding ? (
            <>
              <UserPicker
                exclude={members ?? []}
                selected={adding}
                onToggle={(u) => setAdding((s) => (s!.includes(u.id) ? s!.filter((x) => x !== u.id) : [...s!, u.id]))}
              />
              <div className="row-actions">
                <button onClick={() => setAdding(null)}>Cancel</button>
                <button
                  className="primary-small"
                  disabled={adding.length === 0}
                  onClick={() => act('conversations.invite', { users: adding.join(',') }, () => {
                    setAdding(null)
                    loadMembers()
                  })}
                >
                  Add
                </button>
              </div>
            </>
          ) : (
            <>
              {!isDm && conv.is_member && !conv.is_archived && (
                <button className="add-people" onClick={() => setAdding([])}>
                  + Add people
                </button>
              )}
              {members === undefined ? (
                <p className="muted">Loading…</p>
              ) : (
                <ul className="pick-list">
                  {members.map((id) => (
                    <li key={id}>
                      <span className="pick-main user-pick">
                        <Avatar user={users[id]} id={id} size={24} />
                        <strong>{displayName(users[id], id)}</strong>
                        <span className="muted small">@{users[id]?.name}</span>
                        {users[id]?.is_bot && <span className="app-badge">APP</span>}
                      </span>
                      {!isDm && !conv.is_general && conv.is_member && id !== store.state.me && (
                        <button onClick={() => act('conversations.kick', { user: id }, loadMembers)}>Remove</button>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      )}

      {tab === 'settings' && (
        <div className="settings">
          <label>
            Channel name
            <div className="inline-edit">
              <input value={name} onChange={(e) => setName(channelSlug(e.target.value))} disabled={!conv.is_member || conv.is_archived} />
              {name !== conv.name && <button onClick={() => act('conversations.rename', { name })}>Rename</button>}
            </div>
          </label>
          <div className="danger-zone">
            {conv.is_member && !conv.is_general && !conv.is_archived && (
              <button
                onClick={() =>
                  act('conversations.leave', {}, () => {
                    store.upsertConv({ ...conv, is_member: false })
                    onClose()
                    open()
                  })
                }
              >
                Leave channel
              </button>
            )}
            {!conv.is_general &&
              (conv.is_archived ? (
                <button onClick={() => act('conversations.unarchive', {})}>Unarchive channel</button>
              ) : (
                conv.is_member && (
                  <button
                    className="danger"
                    onClick={() => confirm(`Archive #${conv.name}? People can still read it, but nobody can post.`) && act('conversations.archive', {})}
                  >
                    Archive channel
                  </button>
                )
              ))}
          </div>
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </Modal>
  )
}

function ConnectionDetails({ onClose }: { onClose: () => void }) {
  const { membership } = useWs()
  const apiBase = `${location.origin}/api/`
  return (
    <Modal title="Connection details" onClose={onClose}>
      <dl className="kv">
        <dt>Web API base URL</dt>
        <dd>
          <code>{apiBase}</code>
        </dd>
        <dt>Team ID</dt>
        <dd>
          <code>{membership.team.id}</code>
        </dd>
        <dt>Your user ID</dt>
        <dd>
          <code>{membership.user.id}</code>
        </dd>
      </dl>
      <p className="muted small">
        Point a Slack SDK here with <code>base_url="{apiBase}"</code> (Python) or <code>slackApiUrl: '{apiBase}'</code> (JS). Creating
        apps and tokens arrives in the next milestone.
      </p>
    </Modal>
  )
}
