import { useState, type FormEvent } from 'react'
import { auth, describeError, type Me } from './api'

export function Login({ onAuthed }: { onAuthed: (me: Me) => void }) {
  const [mode, setMode] = useState<'login' | 'signup'>('login')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)

  async function submit(e: FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError(undefined)
    try {
      onAuthed(mode === 'login' ? await auth.login(email, password) : await auth.signup(email, password, displayName))
    } catch (err) {
      setError(describeError(err))
    } finally {
      setBusy(false)
    }
  }

  const signup = mode === 'signup'
  return (
    <div className="centered-page">
      <form className="card auth-card" onSubmit={submit}>
        <div className="brand">
          <span className="brand-mark" aria-hidden>#</span> slack-emu
        </div>
        <h1>{signup ? 'Create your account' : 'Sign in'}</h1>
        {signup && (
          <label>
            Full name
            <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} autoComplete="name" />
          </label>
        )}
        <label>
          Email
          <input type="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoComplete="email" autoFocus />
        </label>
        <label>
          Password
          <input
            type="password"
            required
            minLength={signup ? 8 : undefined}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete={signup ? 'new-password' : 'current-password'}
          />
        </label>
        {error && <p className="error" role="alert">{error}</p>}
        <button className="primary" disabled={busy}>
          {signup ? 'Create account' : 'Sign in'}
        </button>
        <p className="muted switch">
          {signup ? 'Already have an account?' : 'New here?'}{' '}
          <button
            type="button"
            className="link"
            onClick={() => {
              setMode(signup ? 'login' : 'signup')
              setError(undefined)
            }}
          >
            {signup ? 'Sign in' : 'Create an account'}
          </button>
        </p>
      </form>
    </div>
  )
}
