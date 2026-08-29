// Static visual fixture of the real markup/styles. NOT extension or E2E evidence.
import { chromium } from '@playwright/test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const extension = new URL('../../extensions/chrome/', import.meta.url);
let html = await readFile(new URL('popup.html', extension), 'utf8');
const css = await readFile(new URL('styles.css', extension), 'utf8');
const logo = await readFile(new URL('logo.svg', extension));
html = html.replace('<script type="module" src="popup.js"></script>', '')
  .replace('<link rel="stylesheet" href="styles.css">', `<style>${css}</style>`)
  .replace('src="logo.svg"', `src="data:image/svg+xml;base64,${logo.toString('base64')}"`);
const output = new URL('../../test-results/chrome-visual-preview/', import.meta.url);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 400, height: 600 }, deviceScaleFactor: 1 });
  await page.setContent(html);
  await page.evaluate(() => {
    document.getElementById('capture').hidden = false;
    for (const [field, value] of Object.entries({ url: 'https://example.com/designing-better-tools?qa=preview#article', title: 'Designing better tools', description: 'Practical ideas for building focused, useful software.', tags: 'design, software, tools' })) document.getElementById(field).value = value;
    document.getElementById('metadata-status').textContent = 'Metadata filled. Review before saving.';
  });
  await page.screenshot({ path: fileURLToPath(new URL('capture.png', output)) });
  const layout = await page.evaluate(() => ({
    viewportWidth: innerWidth, scrollWidth: document.documentElement.scrollWidth,
    saveVisible: document.getElementById('save-button').getBoundingClientRect().bottom <= innerHeight,
    labels: [...document.querySelectorAll('#capture input, #capture textarea')].every(input => input.labels.length > 0),
  }));
  assert.equal(layout.scrollWidth, layout.viewportWidth, 'no horizontal overflow');
  assert.equal(layout.saveVisible, true, 'primary action is visible');
  assert.equal(layout.labels, true, 'all fields have labels');
  const focusOrder = [];
  for (let index = 0; index < 9; index++) {
    await page.keyboard.press('Tab');
    focusOrder.push(await page.evaluate(() => document.activeElement.id));
  }
  assert.deepEqual(focusOrder, ['settings-button', 'close-button', 'url', 'autofill-button', 'title', 'description', 'tags', 'cancel-button', 'save-button']);
  console.log(JSON.stringify({ ...layout, focusOrder, evidence: 'static markup/style fixture only' }));
} finally { await browser.close(); }
