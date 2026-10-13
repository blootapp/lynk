import { defineConfig } from 'vite'

export default defineConfig({
  root: 'src',
  publicDir: false,
  build: {
    outDir: '../dist',
    emptyOutDir: true,
  },
  server: {
    port: 1430,
    strictPort: true,
  },
  clearScreen: false,
})
