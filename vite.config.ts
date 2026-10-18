import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

/** Trees the UI never imports. Watching them (disposable worktrees especially) exhausts the inotify limit with ENOSPC (FOR-59). */
export const unwatched = ['**/.claude/**', '**/.crewroom/**', '**/.lavish/**', '**/target/**', '**/dist/**', '**/crates/**',
  '**/vendor/**', '**/playwright-report/**', '**/test-results/**'];

export default defineConfig({ plugins: [react()], server: { host: '127.0.0.1', watch: { ignored: unwatched } } });
