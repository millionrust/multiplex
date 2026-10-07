export default defineNuxtRouteMiddleware(async (to) => {
  if (to.path === '/login') return
  const request = useRequestFetch()
  const access = await request<{ authorized: boolean }>('/api/auth/status')
  if (!access.authorized) return navigateTo({ path: '/login', query: { next: to.fullPath } })
})
