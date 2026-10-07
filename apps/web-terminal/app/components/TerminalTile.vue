<script setup lang="ts">
import { Maximize2, Pin, PinOff, Trash2, X } from 'lucide-vue-next'
import type { DropZone, LayoutLeaf } from '../composables/useTileLayout'
import { useViewerContext } from '../composables/viewerContext'

const props = defineProps<{ leaf: LayoutLeaf }>()
const viewer = useViewerContext()

const card = ref<HTMLElement | null>(null)
const view = ref<{ measureFit: () => { cols: number, rows: number } | null } | null>(null)
const paused = ref(false)
const dropZone = ref<DropZone | null>(null)
const sizeOpen = ref(false)
const sizeCols = ref(80)
const sizeRows = ref(24)
const resizing = ref(false)

const pane = computed(() => viewer.paneById(props.leaf.paneId))
const focused = computed(() => viewer.focusedKey.value === props.leaf.key)
const interactive = computed(() => focused.value && viewer.inputEnabled.value)
const snapshot = computed(() => viewer.snapshots[props.leaf.paneId] ?? null)

// A different pane in the same tile starts unpaused.
watch(() => props.leaf.paneId, () => {
  paused.value = false
  sizeOpen.value = false
})

function onDragStart(event: DragEvent) {
  if (!event.dataTransfer) return
  event.dataTransfer.effectAllowed = 'move'
  event.dataTransfer.setData('text/plain', props.leaf.paneId)
  viewer.draggingPaneId.value = props.leaf.paneId
}

// The nearest edge within 30% of the tile splits it on that side; the middle swaps or replaces.
function zoneAt(event: DragEvent): DropZone | null {
  if (!card.value) return null
  const rect = card.value.getBoundingClientRect()
  const x = (event.clientX - rect.left) / rect.width
  const y = (event.clientY - rect.top) / rect.height
  const edges: [number, DropZone][] = [[x, 'left'], [1 - x, 'right'], [y, 'top'], [1 - y, 'bottom']]
  const [distance, zone] = edges.reduce((nearest, edge) => edge[0] < nearest[0] ? edge : nearest)
  return distance < 0.3 ? zone : 'center'
}

function onDragOver(event: DragEvent) {
  const dragging = viewer.draggingPaneId.value
  if (!dragging || dragging === props.leaf.paneId) return
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
  dropZone.value = zoneAt(event)
}

function onDragLeave(event: DragEvent) {
  if (!card.value?.contains(event.relatedTarget as Node | null)) dropZone.value = null
}

function onDrop(event: DragEvent) {
  const dragging = viewer.draggingPaneId.value
  const zone = dropZone.value ?? zoneAt(event)
  dropZone.value = null
  if (!dragging || dragging === props.leaf.paneId || !zone) return
  event.preventDefault()
  viewer.drop(dragging, props.leaf.key, zone)
  viewer.draggingPaneId.value = null
}

watch(() => viewer.draggingPaneId.value, (dragging) => {
  if (!dragging) dropZone.value = null
})

async function resize(size: { cols: number, rows: number } | 'auto') {
  if (resizing.value) return
  resizing.value = true
  try {
    await viewer.resizePane(props.leaf.paneId, size)
    sizeOpen.value = false
  } finally {
    resizing.value = false
  }
}

function fitToTile() {
  const fit = view.value?.measureFit()
  if (fit) void resize({ cols: Math.max(20, Math.min(500, fit.cols)), rows: Math.max(5, Math.min(200, fit.rows)) })
}

function toggleSize() {
  sizeOpen.value = !sizeOpen.value
  if (sizeOpen.value && pane.value) {
    sizeCols.value = snapshot.value?.width ?? pane.value.width
    sizeRows.value = snapshot.value?.height ?? pane.value.height
  }
}

function applySize() {
  void resize({ cols: Math.round(sizeCols.value), rows: Math.round(sizeRows.value) })
}

function onOutsidePointer(event: PointerEvent) {
  if (!(event.target as HTMLElement | null)?.closest('.size-control')) sizeOpen.value = false
}

watch(sizeOpen, (open) => {
  if (open) window.addEventListener('pointerdown', onOutsidePointer, true)
  else window.removeEventListener('pointerdown', onOutsidePointer, true)
})
onBeforeUnmount(() => window.removeEventListener('pointerdown', onOutsidePointer, true))
</script>

<template>
  <div
    ref="card" class="terminal-card" :class="{ focused: focused && viewer.tileCount.value > 1 }" :data-pane-id="leaf.paneId"
    @pointerdown.capture="viewer.focus(leaf.key)" @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop"
  >
    <template v-if="pane">
      <div class="terminal-toolbar" draggable="true" title="Drag to move this terminal" @dragstart="onDragStart" @dragend="viewer.draggingPaneId.value = null">
        <div class="terminal-identity"><strong>{{ viewer.displayName(pane) }}</strong><span class="terminal-meta">{{ pane.id.slice(0, 8) }}</span></div>
        <div class="terminal-details">
          <span class="command-pill">{{ pane.command }}</span>
          <div class="size-control">
            <button type="button" class="terminal-size" :aria-expanded="sizeOpen" aria-label="Resize terminal" title="Resize terminal" @click="toggleSize">{{ snapshot?.width || pane.width }}×{{ snapshot?.height || pane.height }}</button>
            <form v-if="sizeOpen" class="size-popover" @submit.prevent="applySize">
              <label>Columns<input v-model.number="sizeCols" type="number" min="20" max="500" required aria-label="Columns"></label>
              <span class="size-times">×</span>
              <label>Rows<input v-model.number="sizeRows" type="number" min="5" max="200" required aria-label="Rows"></label>
              <button type="submit" class="size-apply" :disabled="resizing">Apply</button>
              <button type="button" class="size-auto" :disabled="resizing" title="Restore the size before browser resizing; other clients can resize normally" @click="resize('auto')">Auto</button>
            </form>
          </div>
          <button type="button" class="tile-button fit-button" :disabled="resizing" :aria-label="focused ? 'Fit terminal to this view' : `Fit ${pane.sessionName} to its tile`" title="Resize the terminal to fill this tile" @click="fitToTile"><Maximize2 :size="11" /></button>
          <button type="button" class="pin-button" :title="pane.pinned ? 'Unpin' : 'Pin'" :aria-pressed="pane.pinned" :aria-label="focused ? (pane.pinned ? 'Unpin selected pane' : 'Pin selected pane') : `${pane.pinned ? 'Unpin' : 'Pin'} ${pane.sessionName} tile`" @click="viewer.togglePin(pane)"><PinOff v-if="pane.pinned" :size="11" /><Pin v-else :size="11" /></button>
          <button type="button" class="kill-button" title="Kill terminal" :aria-label="focused ? 'Kill selected pane' : `Kill ${pane.sessionName} tile`" :disabled="viewer.killingIds.has(pane.id)" @click="viewer.killPane(pane.id)"><Trash2 :size="11" /></button>
          <button v-if="viewer.tileCount.value > 1" type="button" class="tile-button close-tile" :aria-label="`Close ${pane.sessionName} tile`" title="Remove from this view (keeps the pane running)" @click="viewer.close(leaf.key)"><X :size="12" /></button>
        </div>
      </div>
      <div class="terminal-surface"><TerminalView ref="view" :key="pane.id" :snapshot="snapshot" :width="pane.width" :height="pane.height" :interactive="interactive" @input="data => viewer.sendInput(pane!.id, data)" @paste="text => viewer.sendInput(pane!.id, text, true)" @paused="value => paused = value" /></div>
      <div v-if="interactive && viewer.touchScreen.value" class="touch-keys" role="toolbar" aria-label="Extra keys">
        <button v-for="key in viewer.touchKeys" :key="key.label" type="button" :aria-label="key.aria || key.label" @pointerdown.prevent @click="viewer.sendInput(pane.id, key.data)">{{ key.label }}</button>
      </div>
      <div class="terminal-foot"><span v-if="paused" class="foot-paused">updates paused while text is selected · click to resume</span><span v-else>{{ interactive ? 'input mode · click to focus, typing follows the Multiplex cursor' : focused ? 'read only · select text to copy' : 'click to focus' }}</span><span>Multiplex pane %{{ pane.id }}</span></div>
    </template>
    <div v-else class="tile-gone">This pane has ended.</div>
    <div v-if="dropZone" class="drop-preview" :class="`drop-${dropZone}`" />
  </div>
</template>
