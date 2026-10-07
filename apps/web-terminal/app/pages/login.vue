<script setup lang="ts">
const code = ref(''), error = ref(''), busy = ref(false)
const route = useRoute()
async function signIn() {
  busy.value = true; error.value = ''
  try {
    await $fetch('/api/auth/login', { method: 'POST', body: { code: code.value } })
    const next = typeof route.query.next === 'string' && route.query.next.startsWith('/') && !route.query.next.startsWith('//') ? route.query.next : '/'
    await navigateTo(next)
  } catch { error.value = 'Could not sign in. Check the access code, or wait if you tried several times.' }
  finally { busy.value = false }
}
</script>
<template>
  <main class="login-page"><form class="login-card" @submit.prevent="signIn">
    <h1>Multiplex terminals</h1><p>Enter the access code shown where the browser viewer is running.</p>
    <label>Access code<input v-model="code" type="password" autocomplete="current-password" required autofocus></label>
    <p v-if="error" role="alert">{{ error }}</p><button type="submit" :disabled="busy">{{ busy ? 'Connecting…' : 'Open terminals' }}</button>
  </form></main>
</template>
<style scoped>
.login-page { min-height: 100dvh; display:grid; place-items:center; padding:24px }
.login-card { width:min(100%,420px); padding:32px; background:var(--panel); border:1px solid var(--line); border-radius:12px }
h1 { font-size:24px; margin:0 0 12px } p { color:var(--muted); line-height:1.6 } label { display:block; margin-top:24px }
input { display:block; width:100%; padding:12px; margin:8px 0 20px; border:1px solid var(--line); background:var(--bg); border-radius:6px }
button { width:100%; padding:12px; background:var(--raised); color:var(--text); border:1px solid var(--line); border-radius:6px; font-weight:600 }
</style>
