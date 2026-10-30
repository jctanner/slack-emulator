import { createContext, useContext } from 'react'
import type { Membership } from './api'
import type { WorkspaceStore } from './store'

export type WorkspaceCtx = {
  store: WorkspaceStore
  membership: Membership
  sendTyping: (channel: string, thread_ts?: string) => void
  typingDone: (channel: string, thread_ts?: string) => void
  /** Navigates within the current workspace. */
  open: (channel?: string, thread?: string) => void
  showDialog: (d: DialogKind) => void
}

export type DialogKind =
  | { kind: 'create-channel' }
  | { kind: 'browse-channels' }
  | { kind: 'new-message' }
  | { kind: 'channel-details'; channel: string; tab?: 'about' | 'members' | 'pinned' | 'settings' }
  | { kind: 'connection' }
  | { kind: 'usergroups' }
  | { kind: 'workspace-settings' }

export const WorkspaceContext = createContext<WorkspaceCtx | null>(null)

export function useWs(): WorkspaceCtx {
  const ctx = useContext(WorkspaceContext)
  if (!ctx) throw new Error('useWs outside WorkspaceContext')
  return ctx
}
