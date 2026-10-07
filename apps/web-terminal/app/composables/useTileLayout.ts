// The focus view is a tree of splits whose leaves are terminals. Each split
// divides its area in two, side by side ('row') or stacked ('column'), so any
// arrangement (two beside each other, one above two, ...) is a nesting of splits.

export type DropZone = 'left' | 'right' | 'top' | 'bottom' | 'center'

export interface LayoutLeaf {
  kind: 'leaf'
  key: string
  paneId: string
}

export interface LayoutSplit {
  kind: 'split'
  key: string
  dir: 'row' | 'column'
  /** Share of the split given to `first`, 0..1. */
  ratio: number
  first: LayoutNode
  second: LayoutNode
}

export type LayoutNode = LayoutLeaf | LayoutSplit

const STORAGE_KEY = 'multiplex-terminal-viewer-layout'
const MIN_RATIO = 0.12

let keyCounter = 0
function newKey(): string {
  keyCounter += 1
  return `n${Date.now().toString(36)}${keyCounter.toString(36)}`
}

function collectLeaves(node: LayoutNode | null, into: LayoutLeaf[] = []): LayoutLeaf[] {
  if (!node) return into
  if (node.kind === 'leaf') into.push(node)
  else {
    collectLeaves(node.first, into)
    collectLeaves(node.second, into)
  }
  return into
}

/** Rebuilds the tree with one node replaced; returning null removes it and its sibling takes the split's place. */
function replaceNode(node: LayoutNode, key: string, replace: (node: LayoutNode) => LayoutNode | null): LayoutNode | null {
  if (node.key === key) return replace(node)
  if (node.kind === 'leaf') return node
  const first = replaceNode(node.first, key, replace)
  const second = replaceNode(node.second, key, replace)
  if (!first) return second
  if (!second) return first
  return first === node.first && second === node.second ? node : { ...node, first, second }
}

function findSplit(node: LayoutNode | null, key: string): LayoutSplit | null {
  if (!node || node.kind === 'leaf') return null
  if (node.key === key) return node
  return findSplit(node.first, key) ?? findSplit(node.second, key)
}

function isLayoutNode(value: unknown, depth = 0): value is LayoutNode {
  if (!value || typeof value !== 'object' || depth > 12) return false
  const node = value as Record<string, unknown>
  if (typeof node.key !== 'string') return false
  if (node.kind === 'leaf') return typeof node.paneId === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(node.paneId)
  return node.kind === 'split'
    && (node.dir === 'row' || node.dir === 'column')
    && typeof node.ratio === 'number' && node.ratio > 0 && node.ratio < 1
    && isLayoutNode(node.first, depth + 1) && isLayoutNode(node.second, depth + 1)
}

export function useTileLayout() {
  const root = ref<LayoutNode | null>(null)
  const focusedKey = ref<string | null>(null)

  const leaves = computed(() => collectLeaves(root.value))
  const focusedLeaf = computed(() => leaves.value.find(leaf => leaf.key === focusedKey.value) ?? leaves.value[0] ?? null)
  const leafFor = (paneId: string) => leaves.value.find(leaf => leaf.paneId === paneId)

  function focus(key: string) {
    if (leaves.value.some(leaf => leaf.key === key)) focusedKey.value = key
  }

  /** Shows a pane: focuses its tile if it is already open, otherwise loads it into the focused tile. */
  function open(paneId: string) {
    const existing = leafFor(paneId)
    if (existing) {
      focusedKey.value = existing.key
    } else if (focusedLeaf.value) {
      focusedLeaf.value.paneId = paneId
      focusedKey.value = focusedLeaf.value.key
    } else {
      const leaf: LayoutLeaf = { kind: 'leaf', key: newKey(), paneId }
      root.value = leaf
      focusedKey.value = leaf.key
    }
  }

  /** Drops a pane on a tile: an edge splits the tile, the centre swaps with or replaces its pane. */
  function drop(paneId: string, targetKey: string, zone: DropZone) {
    const target = leaves.value.find(leaf => leaf.key === targetKey)
    const source = leafFor(paneId)
    if (!target || !root.value || source?.key === targetKey) return

    if (zone === 'center') {
      if (source) source.paneId = target.paneId
      target.paneId = paneId
      focusedKey.value = target.key
      return
    }

    let tree: LayoutNode | null = root.value
    if (source) tree = replaceNode(tree, source.key, () => null)
    if (!tree) return
    const added: LayoutLeaf = { kind: 'leaf', key: newKey(), paneId }
    const before = zone === 'left' || zone === 'top'
    root.value = replaceNode(tree, targetKey, existing => ({
      kind: 'split',
      key: newKey(),
      dir: zone === 'left' || zone === 'right' ? 'row' : 'column',
      ratio: 0.5,
      first: before ? added : existing,
      second: before ? existing : added,
    }))
    focusedKey.value = added.key
  }

  /** Removes a tile from the layout (the pane keeps running). The last tile stays. */
  function close(key: string) {
    if (!root.value || leaves.value.length <= 1) return
    root.value = replaceNode(root.value, key, () => null)
    if (!leaves.value.some(leaf => leaf.key === focusedKey.value)) focusedKey.value = leaves.value[0]?.key ?? null
  }

  function setRatio(splitKey: string, ratio: number) {
    const split = findSplit(root.value, splitKey)
    if (split) split.ratio = Math.min(1 - MIN_RATIO, Math.max(MIN_RATIO, ratio))
  }

  /** Drops tiles whose pane no longer exists. */
  function prune(validPaneIds: Set<string>) {
    for (const leaf of leaves.value) {
      if (!validPaneIds.has(leaf.paneId) && root.value) root.value = replaceNode(root.value, leaf.key, () => null)
    }
    if (!leaves.value.some(leaf => leaf.key === focusedKey.value)) focusedKey.value = leaves.value[0]?.key ?? null
  }

  function restore() {
    try {
      const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null')
      if (!isLayoutNode(saved?.root)) return
      const panes = collectLeaves(saved.root).map(leaf => leaf.paneId)
      if (new Set(panes).size !== panes.length) return
      root.value = saved.root
      focusedKey.value = typeof saved.focusedKey === 'string' ? saved.focusedKey : null
    } catch {}
  }

  function persist() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ root: root.value, focusedKey: focusedKey.value }))
    } catch {}
  }

  watch([root, focusedKey], persist, { deep: true })

  return { root, focusedKey, leaves, focusedLeaf, leafFor, focus, open, drop, close, setRatio, prune, restore }
}
