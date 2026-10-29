import { useEffect, useState } from 'react'
import { ApiError, auth, type Me } from './api'
import { navigate, teamFromPath, usePath } from './router'
import { Login } from './Login'
import { Client } from './Client'
import { WorkspaceDialog } from './WorkspaceDialog'

export function App() {
  // undefined while loading, null when signed out.
  const [me, setMe] = useState<Me | null | undefined>(undefined)
  const path = usePath()

  const refresh = () =>
    auth.me().then(setMe, (e) => {
      if (e instanceof ApiError && e.status === 401) setMe(null)
      else setTimeout(refresh, 2000)
    })

  useEffect(() => {
    refresh()
  }, [])

  const teamId = teamFromPath(path)
  const membership = me?.workspaces.find((w) => w.team.id === teamId)

  useEffect(() => {
    if (me && !membership && me.workspaces.length > 0) {
      navigate(`/client/${me.workspaces[0].team.id}`, true)
    }
  }, [me, membership])

  if (me === undefined) return <div className="splash" aria-busy="true" />
  if (me === null) return <Login onAuthed={setMe} />

  if (me.workspaces.length === 0) {
    return (
      <div className="centered-page">
        <WorkspaceDialog
          title="You’re not in any workspaces yet"
          onDone={(m) => {
            refresh().then(() => navigate(`/client/${m.team.id}`))
          }}
        />
      </div>
    )
  }
  if (!membership) return <div className="splash" aria-busy="true" />

  return (
    <Client
      me={me}
      membership={membership}
      onWorkspacesChanged={refresh}
      onSignedOut={() => {
        setMe(null)
        navigate('/', true)
      }}
    />
  )
}
