import type { MultiplexTerminal } from '../../shared/types'
// Titles are stored in the CLI record, shared with desktop, mobile, and every browser.
export function usePaneNames() {
  const customName = (_pane: MultiplexTerminal): string | undefined => undefined
  async function rename(pane: MultiplexTerminal, name: string) {
    await $fetch(`/api/panes/${pane.id}/rename`, { method: 'POST', body: { name } })
  }
  return { customName, rename, prune: (_panes: MultiplexTerminal[]) => {}, restore: () => {} }
}
