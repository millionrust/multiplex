<script setup lang="ts">
import { Terminal } from '@xterm/xterm'
import type { SnapshotResponse } from '../../shared/types'

const props = defineProps<{
  snapshot: SnapshotResponse | null
  interactive: boolean
  width: number
  height: number
}>()
const emit = defineEmits<{ input: [data: string], paste: [text: string], paused: [paused: boolean] }>()
const host = ref<HTMLElement | null>(null)
const inner = ref<HTMLElement | null>(null)

let terminal: Terminal | undefined
let lastContent = ''
let lastPosition = ''
let firstRender = true
let observer: ResizeObserver | undefined
let wheelRemainder = 0
let selecting = false
const BASE_FONT = 13
const MIN_FONT = 9
let paused = false

const cmdKeys: Record<string, string> = {
  ArrowLeft: '\x01', // start of line (Ctrl+A)
  ArrowRight: '\x05', // end of line (Ctrl+E)
  Backspace: '\x15', // delete to start of line (Ctrl+U)
  Delete: '\x0b', // delete to end of line (Ctrl+K)
}

function onWheel(event: WheelEvent) {
  if (!terminal) return
  event.preventDefault()
  event.stopPropagation()
  // Trackpads emit many tiny deltas; accumulate pixels and scroll whole rows only.
  const screen = inner.value?.querySelector<HTMLElement>('.xterm-screen')
  const cellHeight = screen && screen.offsetHeight > 0 ? screen.offsetHeight / terminal.rows : 18
  const pixels = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? event.deltaY * cellHeight
    : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? event.deltaY * terminal.rows * cellHeight
      : event.deltaY
  if (Math.sign(pixels) !== Math.sign(wheelRemainder)) wheelRemainder = 0
  wheelRemainder += pixels
  const lines = Math.trunc(wheelRemainder / cellHeight)
  if (!lines) return
  wheelRemainder -= lines * cellHeight
  terminal.scrollLines(lines)
}

// Pastes bypass xterm so the server can paste through Multiplex, which adds
// bracketed-paste markers only when the pane's app asked for them.
function onPaste(event: ClipboardEvent) {
  if (!props.interactive) return
  event.preventDefault()
  event.stopPropagation()
  const text = event.clipboardData?.getData('text/plain')
  if (text) emit('paste', text)
}

function syncCanvasBounds(cursorY: number, rows: number, followCursor: boolean) {
  if (!host.value || !inner.value) return
  const screen = inner.value.querySelector<HTMLElement>('.xterm-screen')
  if (!screen) return
  inner.value.style.width = `${Math.max(host.value.clientWidth, screen.offsetWidth)}px`
  inner.value.style.height = `${screen.offsetHeight}px`
  if (followCursor) {
    const cellHeight = screen.offsetHeight / rows
    host.value.scrollTop = Math.max(0, cursorY * cellHeight - host.value.clientHeight * .72)
  }
}

// On phone-width screens, shrink the font (never below MIN_FONT) so more of the
// pane's fixed column count fits; anything still wider scrolls sideways.
function fitFontSize(cols: number) {
  if (!terminal || !host.value) return
  const screen = inner.value?.querySelector<HTMLElement>('.xterm-screen')
  if (!screen?.offsetWidth) return
  const current = terminal.options.fontSize ?? BASE_FONT
  let next = BASE_FONT
  if (window.matchMedia('(max-width: 760px)').matches) {
    const cellPerFontPx = screen.offsetWidth / terminal.cols / current
    const fitted = Math.floor(host.value.clientWidth / cols / cellPerFontPx * 2) / 2
    next = Math.max(MIN_FONT, Math.min(BASE_FONT, fitted))
  }
  if (next !== current) terminal.options.fontSize = next
}

// Columns and rows that would exactly fill the visible area at the normal font size.
function measureFit(): { cols: number, rows: number } | null {
  if (!terminal || !host.value) return null
  const screen = inner.value?.querySelector<HTMLElement>('.xterm-screen')
  if (!screen?.offsetWidth || !screen.offsetHeight) return null
  const scale = BASE_FONT / (terminal.options.fontSize ?? BASE_FONT)
  const cellWidth = screen.offsetWidth / terminal.cols * scale
  const cellHeight = screen.offsetHeight / terminal.rows * scale
  return { cols: Math.floor(host.value.clientWidth / cellWidth), rows: Math.floor(host.value.clientHeight / cellHeight) }
}

defineExpose({ measureFit })

// Dev builds expose the terminal that has input focus for the Playwright e2e suite.
function exposeForTests() {
  if (import.meta.dev && terminal) (window as unknown as { __multiplexViewerTerminal?: Terminal }).__multiplexViewerTerminal = terminal
}

function onPointerDown(event: PointerEvent) {
  if (event.button !== 0) return
  selecting = true
  window.addEventListener('pointerup', onPointerUp, { once: true })
}

// Every snapshot rewrites the history, and xterm shifts or drops a selection
// when that happens. Rendering therefore pauses mid-drag and while text stays
// selected, so Cmd+C copies exactly what is highlighted; it resumes on release
// without a selection, or once the selection is cleared.
function onPointerUp() {
  selecting = false
  renderSnapshot(props.snapshot)
}

function renderSnapshot(snapshot: SnapshotResponse | null) {
  if (!terminal || selecting || terminal.hasSelection()) return
  if (!snapshot) {
    terminal.reset()
    lastContent = ''
    lastPosition = ''
    firstRender = true
    return
  }

  const cols = Math.max(1, snapshot.width || props.width)
  fitFontSize(cols)
  const paneRows = Math.max(1, snapshot.height || props.height)
  const currentScreen = inner.value?.querySelector<HTMLElement>('.xterm-screen')
  const cellHeight = currentScreen && currentScreen.offsetHeight > 0
    ? currentScreen.offsetHeight / terminal.rows
    : 18
  const fittedRows = host.value ? Math.floor(host.value.clientHeight / cellHeight) : paneRows
  const rows = Math.max(paneRows, fittedRows)
  const cursorX = Math.max(0, Math.min(cols - 1, snapshot.cursorX ?? 0))
  const paneCursorY = Math.max(0, Math.min(paneRows - 1, snapshot.cursorY ?? 0))
  const cursorY = rows - paneRows + paneCursorY
  const position = `${cols}:${rows}:${paneRows}:${cursorX}:${cursorY}:${snapshot.cursorVisible !== false}`
  if (snapshot.content === lastContent && position === lastPosition) return
  const sizeChanged = terminal.cols !== cols || terminal.rows !== rows
  const cursorMoved = position !== lastPosition
  const followCursor = firstRender || props.interactive && cursorMoved
  const distanceFromBottom = Math.max(0, terminal.buffer.active.baseY - terminal.buffer.active.viewportY)

  if (sizeChanged) terminal.resize(cols, rows)
  // capture-pane ends with a newline after the final row. Writing it would scroll
  // the reconstructed screen one row and move the caret away from the terminal's cursor.
  // Trailing blanks (kept by capture -N for their background colour) become
  // erase-to-end-of-line, which paints the same colour without writing cells:
  // a row padded to the exact width wraps and scrolls whenever xterm measures
  // a glyph wider than Multiplex did, and each scroll shifts the user's selection.
  const captured = snapshot.content.replace(/\r?\n$/, '').replace(/ +(?=\r?\n|$)/g, '\x1b[K')
  const capturedRows = captured.split('\n').length
  const content = '\n'.repeat(Math.max(0, rows - capturedRows)) + captured
  const cursor = `\x1b[${cursorY + 1};${cursorX + 1}H\x1b[?25${snapshot.cursorVisible === false ? 'l' : 'h'}`
  terminal.write(`\x1b[3J\x1b[0m\x1b[H\x1b[2J${content}${cursor}`, () => {
    terminal?.scrollToBottom()
    if (!followCursor && distanceFromBottom > 0) {
      terminal?.scrollLines(-Math.min(distanceFromBottom, terminal.buffer.active.baseY))
    }
    requestAnimationFrame(() => syncCanvasBounds(cursorY, rows, followCursor))
  })
  lastContent = snapshot.content
  lastPosition = position
  firstRender = false
}

onMounted(() => {
  if (!inner.value) return
  terminal = new Terminal({
    allowTransparency: false,
    convertEol: true,
    cursorBlink: false,
    disableStdin: !props.interactive,
    cols: Math.max(1, props.width),
    rows: Math.max(1, props.height),
    fontFamily: '"IBM Plex Mono", ui-monospace, monospace',
    fontSize: BASE_FONT,
    lineHeight: 1.36,
    letterSpacing: 0,
    scrollback: 400,
    theme: {
      background: '#17191D',
      foreground: '#E6E8EB',
      cursor: '#74A7F2',
      black: '#263134',
      red: '#d98180',
      green: '#9fc5a8',
      yellow: '#d7bd86',
      blue: '#8faec8',
      magenta: '#bea5c4',
      cyan: '#92c4c1',
      white: '#e2e8e4',
      brightBlack: '#6d7b7c',
      brightRed: '#efa49f',
      brightGreen: '#b6d8b9',
      brightYellow: '#ebd3a0',
      brightBlue: '#abc8dc',
      brightMagenta: '#d1b8d6',
      brightCyan: '#a8d8d2',
      brightWhite: '#f5f8f5',
    },
  })
  terminal.open(inner.value)
  if (props.interactive || !(window as unknown as { __multiplexViewerTerminal?: Terminal }).__multiplexViewerTerminal) exposeForTests()
  terminal.onData(data => {
    if (props.interactive) emit('input', data)
  })
  terminal.onSelectionChange(() => {
    const hasSelection = terminal?.hasSelection() ?? false
    if (hasSelection === paused) return
    paused = hasSelection
    emit('paused', paused)
    if (!paused) renderSnapshot(props.snapshot)
  })
  // macOS line-editing shortcuts, as in Terminal.app and iTerm. Other Cmd
  // combos (copy, paste, select all, reload) stay with the browser.
  terminal.attachCustomKeyEventHandler((event) => {
    if (!event.metaKey || event.ctrlKey || event.altKey) return true
    const sequence = cmdKeys[event.key]
    if (!sequence) return true
    if (event.type === 'keydown') {
      event.preventDefault()
      if (props.interactive) emit('input', sequence)
    }
    return false
  })
  observer = new ResizeObserver(() => renderSnapshot(props.snapshot))
  if (host.value) observer.observe(host.value)
  renderSnapshot(props.snapshot)
  // Ready to type as soon as a terminal opens, except on touch screens, where
  // focusing would pop up the on-screen keyboard on every pane switch.
  if (props.interactive && !window.matchMedia('(pointer: coarse)').matches) terminal.focus()
})

watch(() => props.snapshot, renderSnapshot)
watch(() => props.interactive, (interactive) => {
  if (!terminal) return
  terminal.options.disableStdin = !interactive
  if (interactive) exposeForTests()
  if (interactive && props.snapshot) {
    syncCanvasBounds(props.snapshot.cursorY ?? 0, props.snapshot.height || props.height, true)
    terminal.focus()
  }
})

onBeforeUnmount(() => {
  observer?.disconnect()
  window.removeEventListener('pointerup', onPointerUp)
  terminal?.dispose()
})
</script>

<template>
  <div ref="host" class="terminal-host" :class="{ 'terminal-interactive': interactive }" :aria-label="interactive ? 'Interactive Multiplex terminal' : 'Read-only Multiplex terminal'" @wheel.capture="onWheel" @paste.capture="onPaste" @pointerdown.capture="onPointerDown">
    <div ref="inner" class="terminal-inner" />
  </div>
</template>
