import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
// The console shares the docs' self-hosted fonts; the dev server must be allowed to read them.
export default defineConfig({ plugins: [sveltekit()], server: { fs: { allow: ['../../docs/assets/fonts'] } } });
