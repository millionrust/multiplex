import tailwindcss from '@tailwindcss/vite'

export default defineNuxtConfig({
  ssr: false,
  compatibilityDate: '2026-09-24',
  modules: ['shadcn-nuxt'],
  shadcn: {
    prefix: '',
    componentDir: './app/components/ui',
  },
  css: [
    '@fontsource-variable/dm-sans/index.css',
    '@fontsource/ibm-plex-mono/400.css',
    '@xterm/xterm/css/xterm.css',
    '~/assets/css/main.css',
    '~/assets/css/slate.css',
  ],
  vite: {
    plugins: [tailwindcss()],
    cacheDir: '.nuxt/vite-cache',
    // Let the dev server answer on this machine's Tailscale MagicDNS name.
    server: { allowedHosts: ['.ts.net'] },
  },
  devtools: { enabled: false },
})
