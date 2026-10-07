export interface MultiplexTerminal {
  id: string
  sessionId: string
  sessionName: string
  windowIndex: number
  paneIndex: number
  command: string
  path: string
  live: boolean
  width: number
  height: number
  pinned: boolean
}

export interface SessionsResponse {
  panes: MultiplexTerminal[]
  updatedAt: string | number
  error?: string
}

export interface SnapshotResponse {
  id: string
  content: string
  updatedAt: string | number
  width?: number
  height?: number
  cursorX?: number
  cursorY?: number
  cursorVisible?: boolean
}
