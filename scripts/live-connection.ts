import type { Page } from '@playwright/test';

/**
 * Chromium's offline emulation leaves established WebSockets open. Route the push channel
 * so connection loss closes the page side and refuses reconnects until it is restored.
 * Install before the page first navigates.
 */
export async function routeLiveConnection(page: Page) {
  let offline = false;
  const sockets = new Set<{ close(): Promise<void> }>();
  await page.routeWebSocket(/\/api\/live$/, ws => {
    if (offline) { void ws.close(); return; }
    ws.connectToServer(); sockets.add(ws);
  });
  return async (online: boolean) => {
    offline = !online; await page.context().setOffline(!online);
    if (!online) { for (const ws of sockets) await ws.close(); sockets.clear(); }
  };
}
