import { test, expect } from 'bun:test'
import { ref, computed, watch } from 'vue'
Object.assign(globalThis, { ref, computed, watch })
const store = new Map<string,string>()
Object.assign(globalThis, { localStorage: { getItem: (key:string)=>store.get(key),setItem:(key:string,value:string)=>store.set(key,value) } })
const { useTileLayout } = await import('../app/composables/useTileLayout')
const a='00000000-0000-4000-8000-000000000001',b='00000000-0000-4000-8000-000000000002',c='00000000-0000-4000-8000-000000000003'
test('UUID terminals split, swap, focus and collapse without killing',()=>{
  const layout=useTileLayout();layout.open(a)
  layout.drop(b,layout.leaves.value[0]!.key,'right')
  expect(layout.leaves.value.map(x=>x.paneId)).toEqual([a,b])
  layout.drop(c,layout.leaves.value[1]!.key,'bottom')
  expect(layout.leaves.value.map(x=>x.paneId)).toEqual([a,b,c])
  layout.focus(layout.leafFor(a)!.key);expect(layout.focusedLeaf.value!.paneId).toBe(a)
  layout.drop(a,layout.leafFor(b)!.key,'center')
  expect(layout.leaves.value.map(x=>x.paneId)).toEqual([b,a,c])
  layout.close(layout.leafFor(c)!.key);expect(layout.leaves.value.length).toBe(2)
  layout.prune(new Set([a]));expect(layout.leaves.value.map(x=>x.paneId)).toEqual([a])
})
test('saved UUID layouts restore across reloads',()=>{
  store.clear();const layout=useTileLayout();layout.open(a);layout.drop(b,layout.leaves.value[0]!.key,'bottom')
  store.set('multiplex-terminal-viewer-layout',JSON.stringify({root:layout.root.value,focusedKey:layout.focusedKey.value}))
  const restored=useTileLayout();restored.restore();expect(restored.leaves.value.map(x=>x.paneId)).toEqual([a,b])
})
