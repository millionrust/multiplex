<script setup lang="ts">
import type { LayoutNode } from '../composables/useTileLayout'
import { useViewerContext } from '../composables/viewerContext'

const props = defineProps<{ node: LayoutNode }>()
const viewer = useViewerContext()
const container = ref<HTMLElement | null>(null)
const dragging = ref(false)

// The divider follows the pointer: its position inside the split becomes the ratio.
function onDividerDown(event: PointerEvent) {
  if (props.node.kind !== 'split' || event.button !== 0) return
  event.preventDefault()
  const divider = event.currentTarget as HTMLElement
  divider.setPointerCapture(event.pointerId)
  dragging.value = true
}

function onDividerMove(event: PointerEvent) {
  if (!dragging.value || props.node.kind !== 'split' || !container.value) return
  const rect = container.value.getBoundingClientRect()
  const ratio = props.node.dir === 'row'
    ? (event.clientX - rect.left) / rect.width
    : (event.clientY - rect.top) / rect.height
  viewer.setRatio(props.node.key, ratio)
}

function onDividerUp() {
  dragging.value = false
}

// Arrow keys nudge the focused divider for keyboard users.
function onDividerKey(event: KeyboardEvent) {
  if (props.node.kind !== 'split') return
  const back = props.node.dir === 'row' ? 'ArrowLeft' : 'ArrowUp'
  const forward = props.node.dir === 'row' ? 'ArrowRight' : 'ArrowDown'
  if (event.key !== back && event.key !== forward) return
  event.preventDefault()
  viewer.setRatio(props.node.key, props.node.ratio + (event.key === forward ? 0.03 : -0.03))
}
</script>

<template>
  <TerminalTile v-if="node.kind === 'leaf'" :key="node.key" :leaf="node" />
  <div v-else ref="container" class="tile-split" :class="[`tile-split-${node.dir}`, { 'tile-split-dragging': dragging }]">
    <div class="tile-split-pane" :style="{ flex: `${node.ratio} 1 0` }"><TileLayout :node="node.first" /></div>
    <div
      class="tile-divider" role="separator" tabindex="0"
      :aria-orientation="node.dir === 'row' ? 'vertical' : 'horizontal'" :aria-valuenow="Math.round(node.ratio * 100)" aria-valuemin="12" aria-valuemax="88"
      aria-label="Resize terminals"
      @pointerdown="onDividerDown" @pointermove="onDividerMove" @pointerup="onDividerUp" @pointercancel="onDividerUp" @keydown="onDividerKey"
    />
    <div class="tile-split-pane" :style="{ flex: `${1 - node.ratio} 1 0` }"><TileLayout :node="node.second" /></div>
  </div>
</template>
