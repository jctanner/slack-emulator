import { useEffect, useId, useMemo, useState, type FormEvent } from 'react'
import { ApiError, type View } from './api'
import { Blocks, plain, useBlockSurface, validateInputs, type Container } from './BlockKit'
import { useWs } from './context'
import { useWorkspace } from './store'

/** The top of the user's modal stack (opened by apps with `views.open`). */
export function Modals() {
  const { store } = useWs()
  const views = useWorkspace(store, (s) => s.modals)
  const top = views.at(-1)
  if (!top) return null
  return <ModalView key={top.id} view={top} depth={views.length} />
}

function ModalView({ view, depth }: { view: View; depth: number }) {
  const ws = useWs()
  const { store } = ws
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  const container = useMemo<Container>(() => ({ type: 'view', view_id: view.id }), [view.id])
  const surface = useBlockSurface(view.blocks, container, view.state?.values)
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [submitting, setSubmitting] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const titleId = useId()
  const mctx = useMemo(() => ({ me: store.state.me, users, convs, onChannel: (id: string) => ws.open(id) }), [store, users, convs, ws])

  // A new version of the view clears old validation messages.
  useEffect(() => setErrors({}), [view.hash])

  // `all`: close the whole stack (×, Escape); otherwise just this view (Cancel / Back).
  const close = (all: boolean) => store.call('emu.views.close', { view_id: view.id, all }).catch(() => {})

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close(true)
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [view.id])

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    const local = validateInputs(view.blocks, surface.values)
    setErrors(local)
    if (Object.keys(local).length) return
    setSubmitting(true)
    setProblem(null)
    try {
      const r = await store.call<{ errors?: Record<string, string> }>('emu.views.submit', {
        view_id: view.id,
        state: { values: surface.values },
      })
      if (r.errors) setErrors(r.errors)
    } catch (err) {
      setProblem(
        err instanceof ApiError && err.code === 'dispatch_failed'
          ? `We had some trouble connecting. Try again? (${err.data.detail ?? 'no answer'})`
          : err instanceof ApiError && err.code === 'invalid_arguments'
            ? `The app answered with an invalid view: ${(err.data.response_metadata?.messages ?? []).join(' ')}`
            : `Couldn’t submit (${err instanceof ApiError ? err.code : 'network error'}).`,
      )
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="overlay">
      <form className="card modal bk-modal" role="dialog" aria-modal="true" aria-labelledby={titleId} onSubmit={submit} noValidate>
        <header className="bk-modal-head">
          {depth > 1 && (
            <button type="button" className="icon-button" aria-label="Back" onClick={() => close(false)}>
              ‹
            </button>
          )}
          <h1 id={titleId}>{plain(view.title)}</h1>
          <button type="button" className="icon-button" aria-label="Close" onClick={() => close(true)}>
            ×
          </button>
        </header>
        <div className="bk-modal-body">
          <Blocks blocks={view.blocks} surface={{ ...surface, mctx, errors }} />
        </div>
        {problem && <div className="bk-warning bk-modal-problem">⚠︎ {problem}</div>}
        <footer className="bk-modal-actions">
          <button type="button" className="bk-button" onClick={() => close(false)}>
            {plain(view.close) || (depth > 1 ? 'Back' : 'Cancel')}
          </button>
          {view.submit && (
            <button type="submit" className="bk-button bk-primary" disabled={submitting || view.submit_disabled} aria-busy={submitting}>
              {plain(view.submit)}
            </button>
          )}
        </footer>
      </form>
    </div>
  )
}
