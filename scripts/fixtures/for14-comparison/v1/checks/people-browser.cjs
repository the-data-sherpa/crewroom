'use strict';
const assert = require('node:assert/strict');
const { mkdirSync, readFileSync } = require('node:fs');
const { createHash } = require('node:crypto');
const { join } = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require('/opt/playwright-core');

(async () => {
  const directory = process.env.SCREENSHOTS || '/tmp/for14-artifacts';
  mkdirSync(directory, { recursive: true });
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] }).catch(error => {
    console.error(error);
    // EX_SOFTWARE distinguishes unavailable browser infrastructure from a
    // completed behavioral assertion. Neither is a passing worker check.
    process.exitCode = 70;
    return null;
  });
  if (!browser) return;
  const observations = [];
  try {
    const context = await browser.newContext({ offline: true });
    const page = await context.newPage();
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 844 });
      await page.goto(pathToFileURL(join(process.cwd(), 'index.html')).href);
      const search = page.getByRole('searchbox', { name: 'Search people', exact: true });
      await search.fill('  GRACE  ');
      await page.waitForFunction(() => [...document.querySelectorAll('#people li')].filter(row => !row.hidden).length === 1);
      assert.deepEqual(await page.locator('#people li:visible').allTextContents(), ['Grace Hopper']);
      await search.fill('no such person');
      await page.getByText('No people match', { exact: true }).waitFor({ state: 'visible' });
      assert.equal(await page.locator('#people li:visible').count(), 0);
      const empty = page.locator('#empty');
      assert.ok(['polite', 'assertive'].includes(await empty.getAttribute('aria-live')) || await empty.getAttribute('role') === 'status');
      await search.press('Escape');
      assert.equal(await search.inputValue(), '');
      assert.equal(await page.locator('#people li:visible').count(), 3);
      assert.equal(await search.evaluate(element => element === document.activeElement), true);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      const screenshot = `people-${width}.png`;
      await page.screenshot({ path: join(directory, screenshot), fullPage: true });
      const bytes = readFileSync(join(directory, screenshot));
      assert.ok(bytes.length <= 1024 * 1024, 'Screenshot exceeds retained artifact allowance');
      observations.push({ width, typing: true, whitespaceAndCase: true, emptyState: true, announcement: true, escape: true, focus: true, noOverflow: true,
        screenshot: { name: screenshot, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), base64: bytes.toString('base64') } });
    }
    console.log(JSON.stringify({ version: 1, checkId: 'browser', surface: 'browser', observations }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
