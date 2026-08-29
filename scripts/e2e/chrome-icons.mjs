// Rasterize the existing Boopmark vector logo for Chrome's PNG icon requirement.
import { chromium } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const icons = new URL('../../extensions/chrome/icons/', import.meta.url);
await mkdir(icons, { recursive: true });
const logo = await readFile(new URL('../../static/boopmark-logo.svg', import.meta.url), 'utf8');
const browser = await chromium.launch();
try {
  for (const size of [16, 32, 48, 128]) {
    const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
    await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${logo}`);
    await page.screenshot({ path: fileURLToPath(new URL(`${size}.png`, icons)), omitBackground: true });
    await page.close();
  }
} finally { await browser.close(); }
