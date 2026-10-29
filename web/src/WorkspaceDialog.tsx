import { useEffect, useState, type FormEvent } from 'react'
import { auth, describeError, type Membership, type Team } from './api'

function slugify(name: string) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 21)
}

/** Join an existing workspace or create a new one. */
export function WorkspaceDialog({
  title = 'Add a workspace',
  onDone,
  onCancel,
}: {
  title?: string
  onDone: (m: Membership) => void
  onCancel?: () => void
}) {
  const [teams, setTeams] = useState<Team[]>()
  const [name, setName] = useState('')
  const [domain, setDomain] = useState('')
  const [domainEdited, setDomainEdited] = useState(false)
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    auth.workspaces().then((r) => setTeams(r.workspaces.filter((t) => !t.is_member)), (e) => setError(describeError(e)))
  }, [])

  async function run(action: () => Promise<Membership>) {
    setBusy(true)
    setError(undefined)
    try {
      onDone(await action())
    } catch (e) {
      setError(describeError(e))
      setBusy(false)
    }
  }

  function create(e: FormEvent) {
    e.preventDefault()
    run(() => auth.createWorkspace(name, domain))
  }

  return (
    <div className="card workspace-dialog">
      <div className="dialog-head">
        <h1>{title}</h1>
        {onCancel && (
          <button className="icon-button" onClick={onCancel} aria-label="Close">
            ×
          </button>
        )}
      </div>

      <section>
        <h2>Join a workspace</h2>
        {teams === undefined ? (
          <p className="muted">Loading…</p>
        ) : teams.length === 0 ? (
          <p className="muted">No other workspaces on this server.</p>
        ) : (
          <ul className="team-list">
            {teams.map((t) => (
              <li key={t.id}>
                <span className="team-avatar small">{initials(t.name)}</span>
                <span className="team-list-name">
                  {t.name}
                  <span className="muted"> · {t.domain}</span>
                </span>
                <button disabled={busy} onClick={() => run(() => auth.joinWorkspace(t.id))}>
                  Join
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <form onSubmit={create}>
        <h2>Create a new workspace</h2>
        <label>
          Name
          <input
            required
            value={name}
            onChange={(e) => {
              setName(e.target.value)
              if (!domainEdited) setDomain(slugify(e.target.value))
            }}
            placeholder="Acme Bots"
          />
        </label>
        <label>
          URL handle
          <input
            required
            value={domain}
            pattern="[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?"
            maxLength={21}
            onChange={(e) => {
              setDomain(e.target.value)
              setDomainEdited(true)
            }}
            placeholder="acme-bots"
          />
        </label>
        {error && <p className="error" role="alert">{error}</p>}
        <button className="primary" disabled={busy}>
          Create workspace
        </button>
      </form>
    </div>
  )
}

export function initials(name: string) {
  const words = name.trim().split(/\s+/).filter(Boolean)
  return (words.length > 1 ? words[0][0] + words[1][0] : name.slice(0, 2)).toUpperCase()
}
