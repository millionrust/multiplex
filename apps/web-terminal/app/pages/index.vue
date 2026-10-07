<script setup lang="ts">
import {
  LogOut, ChevronDown, Columns2, Grid2X2, Keyboard, LockKeyhole, Monitor, Moon, Pin, PinOff,
  PanelLeftClose, PanelLeftOpen, RefreshCw, Search, Sun, TerminalSquare, Trash2,
} from 'lucide-vue-next'
import type { SessionsResponse, SnapshotResponse, MultiplexTerminal } from '../../shared/types'
import { viewerContextKey } from '../composables/viewerContext'

const THEME_KEY = 'multiplex-terminal-viewer-theme'

useHead({
  title: 'Multiplex terminals',
  meta: [{ name: 'description', content: 'A live, read-only view of Multiplex CLI terminals.' }],
  // Set the theme before first paint so a dark preference never flashes light.
  script: [{
    tagPosition: 'head',
    innerHTML: `(function(){var t;try{t=localStorage.getItem('${THEME_KEY}')}catch(e){}if(t!=='light'&&t!=='dark')t=matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light';document.documentElement.dataset.theme=t})()`,
  }],
})

const route = useRoute()
const router = useRouter()

// The open terminal lives in ?termId= (the Multiplex pane number) so reloads and
// shared links reopen it.
function queryTermId(): string | null {
  const value = route.query.termId
  return typeof value === 'string' && /^[0-9a-f-]{36}$/i.test(value) ? value : null
}

const layout = useTileLayout()
const paneNames = usePaneNames()

const panes = ref<MultiplexTerminal[]>([])
// The focused tile's pane: it takes keyboard input and is the one in ?termId=.
const selectedId = computed(() => layout.focusedLeaf.value?.paneId ?? null)
// Pane asked for by the URL, opened once the first session list arrives.
let requestedTermId = queryTermId()
const snapshots = reactive<Record<string, SnapshotResponse | null>>({})
const draggingPaneId = ref<string | null>(null)
const editingId = ref<string | null>(null)
const editingName = ref('')
// Phone-width screens show only the focused terminal.
const narrow = ref(false)
// Opening a terminal starts in input mode; the toggle switches to read only.
const inputEnabled = ref(true)
const previews = reactive<Record<string, string>>({})
const search = ref('')
const viewMode = ref<'focus' | 'grid'>('focus')
const sidebarOpen = ref(true)
const loading = ref(true)
const refreshing = ref(false)
const error = ref('')
const actionError = ref('')
const updatedAt = ref<Date | null>(null)
const now = ref(Date.now())
const killingIds = reactive(new Set<string>())
const theme = ref<'light' | 'dark'>('light')
const searchInput = ref<HTMLInputElement | null>(null)
const touchScreen = ref(false)

// Keys phone keyboards lack, shown under the terminal in input mode on touch screens.
const touchKeys = [
  { label: 'Esc', data: '\x1b' },
  { label: 'Tab', data: '\t' },
  { label: '⇧Tab', data: '\x1b[Z' },
  { label: '^C', data: '\x03', aria: 'Control C' },
  { label: '↑', data: '\x1b[A', aria: 'Up' },
  { label: '↓', data: '\x1b[B', aria: 'Down' },
  { label: '←', data: '\x1b[D', aria: 'Left' },
  { label: '→', data: '\x1b[C', aria: 'Right' },
  { label: '⏎', data: '\r', aria: 'Enter' },
]

let sessionsTimer: ReturnType<typeof setInterval> | undefined
let contentTimer: ReturnType<typeof setInterval> | undefined
let clockTimer: ReturnType<typeof setInterval> | undefined
let inputRefreshTimer: ReturnType<typeof setTimeout> | undefined
let fetchingSessions = false
let fetchingContent = false
let contentRefreshRequested = false
let inputQueue: Promise<void> = Promise.resolve()
let contentTick = 0

const selectedPane = computed(() => panes.value.find(pane => pane.id === selectedId.value))
const paneById = (id: string) => panes.value.find(pane => pane.id === id)
const openPaneIds = computed(() => new Set(layout.leaves.value.map(leaf => leaf.paneId)))
const visibleRoot = computed(() => narrow.value ? layout.focusedLeaf.value : layout.root.value)
const visiblePaneIds = computed(() => narrow.value
  ? (selectedId.value ? [selectedId.value] : [])
  : layout.leaves.value.map(leaf => leaf.paneId))
const filteredPanes = computed(() => {
  const query = search.value.trim().toLowerCase()
  return panes.value.filter(pane => !query || [pane.sessionName, pane.command, pane.path, paneNames.customName(pane) ?? ''].some(value => value.toLowerCase().includes(query)))
})
// Pinned panes get their own group at the top; the rest group by project.
const groupedPanes = computed(() => {
  const pinned = filteredPanes.value.filter(pane => pane.pinned)
  const rest = filteredPanes.value.filter(pane => !pane.pinned)
  return [
    ...(pinned.length ? [{ key: ':pinned', label: 'Pinned', items: pinned }] : []),
    { key: ':sessions', label: 'Sessions', items: rest },
  ]
})
const projectCount = computed(() => new Set(filteredPanes.value.map(pane => projectName(pane.sessionName))).size)
const gridPanes = computed(() => [...filteredPanes.value].sort((a, b) => Number(b.pinned) - Number(a.pinned)))
const updatedLabel = computed(() => {
  if (!updatedAt.value) return 'Waiting for Multiplex'
  const seconds = Math.max(0, Math.floor((now.value - updatedAt.value.getTime()) / 1000))
  return seconds < 5 ? 'Updated just now' : `Updated ${seconds}s ago`
})

function projectName(sessionName: string) {
  return sessionName
}

// The user's own name for a terminal if they gave one (stored in this browser only).
const displayName = (pane: MultiplexTerminal) => paneNames.customName(pane) ?? pane.sessionName
const shortName = (pane: MultiplexTerminal) => paneNames.customName(pane) ?? projectName(pane.sessionName)

function paneTag(pane: MultiplexTerminal) {
  return pane.sessionName.match(/-(\d+)$/)?.[1] || pane.id.slice(0, 8)
}

function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.dataset.theme = theme.value
  try { localStorage.setItem(THEME_KEY, theme.value) } catch {}
}

function selectPane(id: string) {
  layout.open(id)
  inputEnabled.value = true
  viewMode.value = 'focus'
  if (window.innerWidth < 760) sidebarOpen.value = false
  void refreshContent(true)
}

function startRename(pane: MultiplexTerminal) {
  editingId.value = pane.id
  editingName.value = paneNames.customName(pane) ?? ''
  void nextTick(() => document.querySelector<HTMLInputElement>('.row-rename')?.select())
}

function finishRename(pane: MultiplexTerminal, save: boolean) {
  if (editingId.value !== pane.id) return
  editingId.value = null
  if (save) void paneNames.rename(pane, editingName.value).then(() => refreshSessions()).catch(cause => { actionError.value = cause.message })
}

function onRowDragStart(event: DragEvent, pane: MultiplexTerminal) {
  if (!event.dataTransfer) return
  event.dataTransfer.effectAllowed = 'move'
  event.dataTransfer.setData('text/plain', pane.id)
  draggingPaneId.value = pane.id
  viewMode.value = 'focus'
}

async function refreshSessions() {
  if (fetchingSessions) return
  fetchingSessions = true
  refreshing.value = true
  try {
    const response = await $fetch<SessionsResponse>('/api/sessions')
    panes.value = response.panes
    error.value = response.error || ''
    updatedAt.value = new Date(response.updatedAt)
    const currentIds = new Set(response.panes.map(pane => pane.id))
    layout.prune(currentIds)
    if (requestedTermId && currentIds.has(requestedTermId)) layout.open(requestedTermId)
    requestedTermId = null
    if (!layout.leaves.value.length && response.panes[0]) {
      layout.open(response.panes[0].id)
      inputEnabled.value = true
    }
    // An empty list can mean Multiplex is briefly unreachable; keep names until panes are known gone.
    if (response.panes.length) paneNames.prune(response.panes)
    for (const id of Object.keys(previews)) if (!currentIds.has(id)) delete previews[id]
    for (const id of Object.keys(snapshots)) if (!openPaneIds.value.has(id)) delete snapshots[id]
  } catch (cause) {
    error.value = cause && typeof cause === 'object' && 'data' in cause && (cause as { data?: { message?: string } }).data?.message ? (cause as { data: { message: string } }).data.message : cause instanceof Error ? cause.message : 'Could not connect to the local viewer'
  } finally {
    fetchingSessions = false
    refreshing.value = false
    loading.value = false
  }
}

async function refreshContent(force = false) {
  if (fetchingContent) {
    if (force) contentRefreshRequested = true
    return
  }
  fetchingContent = true
  try {
    if (viewMode.value === 'focus') {
      // The focused terminal refreshes every tick; the others every fourth
      // tick while typing (about once a second), to keep Multiplex calls bounded.
      contentTick += 1
      const focusedId = selectedId.value
      const due = visiblePaneIds.value.filter(id => id === focusedId || force || !snapshots[id] || !inputEnabled.value || contentTick % 4 === 0)
      await Promise.all(due.map(async (id) => {
        try {
          snapshots[id] = await $fetch<SnapshotResponse>(`/api/panes/${id}`)
        } catch {
          snapshots[id] = { id, content: 'Pane ended or could not be captured.', updatedAt: new Date().toISOString() }
        }
      }))
    } else if (viewMode.value === 'grid') {
      // Keep requests bounded on hosts with many sessions.
      const ids = filteredPanes.value.map(pane => pane.id)
      for (let offset = 0; offset < ids.length; offset += 6) {
        await Promise.all(ids.slice(offset, offset + 6).map(async (id) => {
          try {
            const response = await $fetch<SnapshotResponse>(`/api/panes/${id}?plain=1`)
            previews[id] = response.content
          } catch {
            previews[id] = 'Pane ended'
          }
        }))
      }
    }
  } finally {
    fetchingContent = false
    if (contentRefreshRequested) {
      contentRefreshRequested = false
      void refreshContent()
    }
  }
}

function restartContentTimer() {
  clearInterval(contentTimer)
  contentTimer = setInterval(() => void refreshContent(), inputEnabled.value ? 250 : 1800)
}

function scheduleInputRefresh() {
  clearTimeout(inputRefreshTimer)
  inputRefreshTimer = setTimeout(() => void refreshContent(true), 45)
}

function setView(mode: 'focus' | 'grid') {
  viewMode.value = mode
  if (mode === 'grid') inputEnabled.value = false
  void refreshContent()
}

function toggleInput() {
  inputEnabled.value = !inputEnabled.value
  actionError.value = ''
  if (inputEnabled.value) void refreshContent(true)
}

function previewText(id: string) {
  const content = previews[id]
  if (!content) return 'Loading terminal output…'
  return content.split('\n').slice(0, 20).join('\n').trimEnd() || 'Terminal is blank'
}

function sendInput(id: string, data: string, paste = false) {
  if (!inputEnabled.value) return
  inputQueue = inputQueue.catch(() => {}).then(async () => {
    await $fetch(`/api/panes/${id}/input`, { method: 'POST', body: { data, paste } })
    actionError.value = ''
    scheduleInputRefresh()
  }).catch((cause) => {
    actionError.value = cause && typeof cause === 'object' && 'data' in cause && (cause as { data?: { message?: string } }).data?.message ? (cause as { data: { message: string } }).data.message : cause instanceof Error ? cause.message : 'Could not send input to the pane'
  })
}

async function togglePin(pane: MultiplexTerminal) {
  const pinned = !pane.pinned
  pane.pinned = pinned
  try {
    await $fetch(`/api/panes/${pane.id}/pin`, { method: 'POST', body: { pinned } })
    actionError.value = ''
  } catch (cause) {
    pane.pinned = !pinned
    actionError.value = cause && typeof cause === 'object' && 'data' in cause && (cause as { data?: { message?: string } }).data?.message ? (cause as { data: { message: string } }).data.message : cause instanceof Error ? cause.message : 'Could not pin the pane'
  }
}

async function resizePane(id: string, size: { cols: number, rows: number } | 'auto') {
  try {
    await $fetch(`/api/panes/${id}/resize`, { method: 'POST', body: size === 'auto' ? { auto: true } : size })
    actionError.value = ''
    await refreshSessions()
    await refreshContent(true)
  } catch (cause) {
    actionError.value = cause && typeof cause === 'object' && 'data' in cause && (cause as { data?: { message?: string } }).data?.message ? (cause as { data: { message: string } }).data.message : cause instanceof Error ? cause.message : 'Could not resize the pane'
  }
}

async function killPane(id: string) {
  if (killingIds.has(id)) return
  killingIds.add(id)
  try {
    await $fetch(`/api/panes/${id}`, { method: 'DELETE' })
    actionError.value = ''
    await refreshSessions()
    await refreshContent()
  } catch (cause) {
    actionError.value = cause && typeof cause === 'object' && 'data' in cause && (cause as { data?: { message?: string } }).data?.message ? (cause as { data: { message: string } }).data.message : cause instanceof Error ? cause.message : 'Could not kill the pane'
  } finally {
    killingIds.delete(id)
  }
}

// "/" jumps to the pane filter unless focus is already in a text field,
// which includes the terminal while input mode is on.
function onGlobalKeydown(event: KeyboardEvent) {
  if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return
  const target = event.target as HTMLElement | null
  if (target?.closest('input, textarea, [contenteditable="true"]')) return
  event.preventDefault()
  sidebarOpen.value = true
  void nextTick(() => searchInput.value?.focus())
}

provide(viewerContextKey, {
  paneById,
  displayName,
  snapshots,
  focusedKey: layout.focusedKey,
  tileCount: computed(() => narrow.value ? 1 : layout.leaves.value.length),
  inputEnabled,
  touchScreen,
  touchKeys,
  killingIds,
  draggingPaneId,
  focus: layout.focus,
  close: layout.close,
  drop: (paneId, tileKey, zone) => {
    layout.drop(paneId, tileKey, zone)
    void refreshContent(true)
  },
  setRatio: layout.setRatio,
  sendInput,
  togglePin,
  killPane,
  resizePane,
})

let narrowQuery: MediaQueryList | undefined
const onNarrowChange = () => { narrow.value = narrowQuery?.matches ?? false }

onMounted(async () => {
  window.addEventListener('keydown', onGlobalKeydown)
  narrowQuery = window.matchMedia('(max-width: 760px)')
  narrowQuery.addEventListener('change', onNarrowChange)
  onNarrowChange()
  layout.restore()
  paneNames.restore()
  touchScreen.value = window.matchMedia('(pointer: coarse)').matches
  theme.value = document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light'
  if (window.innerWidth < 760) sidebarOpen.value = false
  await refreshSessions()
  await refreshContent()
  sessionsTimer = setInterval(refreshSessions, 3000)
  restartContentTimer()
  clockTimer = setInterval(() => { now.value = Date.now() }, 1000)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
  narrowQuery?.removeEventListener('change', onNarrowChange)
  clearInterval(sessionsTimer)
  clearInterval(contentTimer)
  clearInterval(clockTimer)
  clearTimeout(inputRefreshTimer)
})

watch(selectedId, (id) => {
  // Before the first session list arrives there is no selection yet; keep the URL's id.
  if (loading.value && !id) return
  void refreshContent(true)
  if ((id ?? null) === queryTermId()) return
  const query = { ...route.query }
  if (id) query.termId = id
  else delete query.termId
  void router.replace({ query })
})

watch(inputEnabled, () => {
  if (import.meta.client) restartContentTimer()
})
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <div class="brand">
        <button type="button" class="icon-button sidebar-toggle" :aria-label="sidebarOpen ? 'Hide sidebar' : 'Show sidebar'" :title="sidebarOpen ? 'Hide sidebar' : 'Show sidebar'" @click="sidebarOpen = !sidebarOpen"><PanelLeftClose v-if="sidebarOpen" :size="15" /><PanelLeftOpen v-else :size="15" /></button>
        <span class="brand-mark"><TerminalSquare :size="13" :stroke-width="2.2" /></span>
        <span class="brand-name">Multiplex terminals</span>
      </div>

      <div class="crumbs">
        <template v-if="viewMode === 'grid'">
          <strong>All terminals</strong><span class="crumb-sep">/</span><span class="crumb-path">{{ filteredPanes.length }} terminals</span>
        </template>
        <template v-else-if="selectedPane">
          <strong>{{ shortName(selectedPane) }}</strong><span class="crumb-sep">/</span><span class="crumb-path" :title="selectedPane.path">{{ selectedPane.path }}</span>
        </template>
        <span v-else class="crumb-path">Select a session to begin</span>
      </div>

      <div class="topbar-actions">
        <button type="button" class="icon-button" aria-label="Sign out" title="Sign out" @click="async () => { await $fetch('/api/auth/logout', { method: 'POST' }); await navigateTo('/login') }"><LogOut :size="13" /></button>
        <span class="pane-count"><span class="live-dot" />{{ panes.length }} terminals</span>
        <button v-if="viewMode === 'focus' && selectedPane" type="button" class="mode-toggle" role="switch" :aria-checked="inputEnabled" :aria-label="inputEnabled ? 'Switch to read only mode' : 'Switch to input mode'" @click="toggleInput"><Keyboard v-if="inputEnabled" :size="12" /><LockKeyhole v-else :size="12" /><span>{{ inputEnabled ? 'Input mode' : 'Read only' }}</span></button>
        <div class="view-switch" role="group" aria-label="View mode">
          <button type="button" :class="{ active: viewMode === 'focus' }" aria-label="Focus view" title="Focus view" @click="setView('focus')"><Columns2 :size="13" /></button>
          <button type="button" :class="{ active: viewMode === 'grid' }" aria-label="Grid view" title="Grid view" @click="setView('grid')"><Grid2X2 :size="13" /></button>
        </div>
        <button type="button" class="icon-button bordered" :aria-label="theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'" :title="theme === 'dark' ? 'Light mode' : 'Dark mode'" @click="toggleTheme"><Sun v-if="theme === 'dark'" :size="14" /><Moon v-else :size="14" /></button>
      </div>
    </header>

    <div class="workspace">
      <div v-if="sidebarOpen" class="mobile-scrim" @click="sidebarOpen = false" />
      <aside class="sidebar" :class="{ 'sidebar-open': sidebarOpen }">
        <label class="search-wrap">
          <Search :size="13" />
          <input ref="searchInput" v-model="search" type="search" placeholder="Filter terminals" aria-label="Filter terminals">
          <kbd>/</kbd>
        </label>

        <nav class="session-list" aria-label="Multiplex sessions">
          <div v-if="loading" class="list-message">Finding Multiplex panes…</div>
          <div v-else-if="!filteredPanes.length" class="list-message">{{ search ? 'No sessions match your search.' : 'No Multiplex sessions are running.' }}</div>
          <section v-for="group in groupedPanes" :key="group.key" class="session-group" :class="{ 'pinned-group': group.key === ':pinned' }">
            <div class="group-label"><span><Pin v-if="group.key === ':pinned'" :size="10" />{{ group.label }}</span><span>{{ group.items.length }}</span></div>
            <div v-for="pane in group.items" :key="pane.id" class="session-item" :class="{ killing: killingIds.has(pane.id), selected: selectedId === pane.id && viewMode === 'focus', open: openPaneIds.has(pane.id) && viewMode === 'focus', editing: editingId === pane.id }">
              <div v-if="editingId === pane.id" class="session-row">
                <span class="row-status" :class="{ live: pane.live }" />
                <input v-model="editingName" class="row-rename" type="text" maxlength="80" :placeholder="projectName(pane.sessionName)" :aria-label="`Name for ${pane.sessionName}`" @keydown.enter.prevent="finishRename(pane, true)" @keydown.esc.prevent="finishRename(pane, false)" @blur="finishRename(pane, true)">
              </div>
              <button v-else class="session-row" type="button" draggable="true" :title="pane.sessionName" @click="selectPane(pane.id)" @dblclick="startRename(pane)" @dragstart="onRowDragStart($event, pane)" @dragend="draggingPaneId = null">
                <span class="row-status" :class="{ live: pane.live }" :title="pane.live ? 'Running' : 'Ended'" />
                <span class="row-name">{{ shortName(pane) }}</span>
                <span class="row-tag">{{ paneTag(pane) }}</span>
              </button>
              <div class="row-actions">
                <button class="row-pin" type="button" :aria-label="`${pane.pinned ? 'Unpin' : 'Pin'} pane ${pane.sessionName}`" :aria-pressed="pane.pinned" :title="pane.pinned ? 'Unpin' : 'Pin to top'" @click="togglePin(pane)"><PinOff v-if="pane.pinned" :size="12" /><Pin v-else :size="12" /></button>
                <button class="row-kill" type="button" :aria-label="`Kill terminal ${pane.sessionName}`" title="Kill terminal" :disabled="killingIds.has(pane.id)" @click="killPane(pane.id)"><Trash2 :size="12" /></button>
              </div>
            </div>
          </section>
        </nav>

        <div class="sidebar-foot"><span class="live-dot" /><span>{{ updatedLabel }}</span><button type="button" class="icon-button" aria-label="Refresh sessions" title="Refresh sessions" @click="refreshSessions"><RefreshCw :size="13" :class="{ spinning: refreshing }" /></button></div>
      </aside>

      <main class="main-panel">
        <div v-if="error || actionError" class="error-banner">{{ actionError || error }}</div>

        <div v-if="viewMode === 'focus'" class="focus-layout" :class="{ 'drag-active': draggingPaneId }">
          <TileLayout v-if="visibleRoot" :node="visibleRoot" />
          <div v-else class="empty-state"><div class="empty-icon"><Monitor :size="22" /></div><h2>No terminal selected</h2><p>Start a Multiplex session and it will appear here automatically.</p></div>
        </div>

        <div v-else class="grid-layout">
          <div v-if="!filteredPanes.length" class="empty-state"><div class="empty-icon"><Monitor :size="22" /></div><h2>No terminals found</h2><p>Start a Multiplex session or change your search.</p></div>
          <div v-else class="terminal-grid">
            <button v-for="pane in gridPanes" :key="pane.id" type="button" class="preview-card" @click="selectPane(pane.id)">
              <div class="preview-head"><span class="preview-name"><Pin v-if="pane.pinned" :size="11" class="preview-pin" />{{ displayName(pane) }}</span><span class="preview-open">Open <ChevronDown :size="12" /></span></div>
              <pre>{{ previewText(pane.id) }}</pre>
              <div class="preview-foot"><span><span class="row-status" :class="{ live: pane.live }" />{{ pane.command }}</span><span>%{{ pane.id }}</span></div>
            </button>
          </div>
        </div>
      </main>
    </div>
  </div>
</template>
