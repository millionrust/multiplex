import type { ComputedRef, InjectionKey, Ref } from 'vue'
import type { SnapshotResponse, MultiplexTerminal } from '../../shared/types'
import type { DropZone } from './useTileLayout'

export interface TouchKey { label: string, data: string, aria?: string }

/** What the page shares with the tiles of the focus view. */
export interface ViewerContext {
  paneById: (id: string) => MultiplexTerminal | undefined
  displayName: (pane: MultiplexTerminal) => string
  snapshots: Record<string, SnapshotResponse | null>
  focusedKey: Ref<string | null>
  tileCount: ComputedRef<number>
  inputEnabled: Ref<boolean>
  touchScreen: Ref<boolean>
  touchKeys: TouchKey[]
  killingIds: Set<string>
  /** Pane being dragged from the sidebar or a tile header, if any. */
  draggingPaneId: Ref<string | null>
  focus: (tileKey: string) => void
  close: (tileKey: string) => void
  drop: (paneId: string, tileKey: string, zone: DropZone) => void
  setRatio: (splitKey: string, ratio: number) => void
  sendInput: (paneId: string, data: string, paste?: boolean) => void
  togglePin: (pane: MultiplexTerminal) => void
  killPane: (paneId: string) => void
  resizePane: (paneId: string, size: { cols: number, rows: number } | 'auto') => Promise<void>
}

export const viewerContextKey: InjectionKey<ViewerContext> = Symbol('multiplex-terminal-viewer')

export function useViewerContext(): ViewerContext {
  const context = inject(viewerContextKey)
  if (!context) throw new Error('Viewer context is missing')
  return context
}
