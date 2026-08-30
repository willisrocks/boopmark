import { chromium } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = resolve(repository, 'extensions/chrome/store-assets');
const popup = await readFile(resolve(repository, 'test-results/chrome-production/postdeploy-autofilled.png'));
const logo = await readFile(resolve(repository, 'extensions/chrome/icons/128.png'));
const popupData = `data:image/png;base64,${popup.toString('base64')}`;
const logoData = `data:image/png;base64,${logo.toString('base64')}`;
await mkdir(output, { recursive: true });

const browser = await chromium.launch();
try {
  const screenshot = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await screenshot.setContent(`<!doctype html><style>
    *{box-sizing:border-box}html,body{margin:0;width:100%;height:100%;overflow:hidden}
    body{font-family:Inter,ui-sans-serif,system-ui,-apple-system,sans-serif;background:radial-gradient(circle at 16% 12%,#2b3659 0,#1e2337 42%,#11141f 100%);color:#f5f7ff;display:flex;align-items:center;padding:70px 80px;gap:72px}
    .copy{width:610px}.brand{display:flex;align-items:center;gap:18px;font-weight:760;font-size:30px}.brand img{width:64px;height:64px}
    h1{font-size:64px;line-height:1.02;letter-spacing:-2.2px;margin:46px 0 24px;max-width:600px}.accent{color:#91bdff}
    p{font-size:24px;line-height:1.48;color:#c7ccdc;margin:0;max-width:565px}
    .chips{display:flex;gap:12px;margin-top:34px}.chip{font-size:17px;background:#2d68ee;color:white;border-radius:999px;padding:11px 17px;font-weight:650}
    .frame{height:670px;width:494px;border-radius:28px;background:#30374f;padding:14px;box-shadow:0 30px 70px #070910aa,0 0 0 1px #64709588;display:flex;align-items:flex-start;justify-content:center;overflow:hidden}
    .frame img{width:466px;height:auto;border-radius:17px;display:block}
  </style><div class="copy"><div class="brand"><img src="${logoData}">Boopmark</div><h1>Save it now.<br><span class="accent">Find it later.</span></h1><p>Capture any page, let Boopmark suggest useful metadata, then review everything before saving.</p><div class="chips"><span class="chip">One click</span><span class="chip">AI autofill</span><span class="chip">Always reviewable</span></div></div><div class="frame"><img src="${popupData}" alt="Boopmark add bookmark popup"></div>`);
  await screenshot.screenshot({ path: resolve(output, 'screenshot-capture-and-review.png') });
  await screenshot.close();

  const tile = await browser.newPage({ viewport: { width: 440, height: 280 } });
  await tile.setContent(`<!doctype html><style>
    *{box-sizing:border-box}html,body{margin:0;width:100%;height:100%;overflow:hidden}body{font-family:Inter,ui-sans-serif,system-ui,-apple-system,sans-serif;background:radial-gradient(circle at 12% 12%,#303b61 0,#1d2236 55%,#11141f 100%);color:#fff;padding:30px 34px;position:relative}.brand{display:flex;align-items:center;gap:12px;font-size:23px;font-weight:760}.brand img{width:48px;height:48px}h1{font-size:39px;line-height:1.04;letter-spacing:-1.2px;margin:28px 0 0;max-width:360px}.accent{color:#91bdff}.dot{position:absolute;width:145px;height:145px;border-radius:50%;background:#2d68ee22;right:-35px;bottom:-55px;border:1px solid #91bdff33}
  </style><div class="brand"><img src="${logoData}">Boopmark</div><h1>Save any page.<br><span class="accent">Keep the context.</span></h1><div class="dot"></div>`);
  await tile.screenshot({ path: resolve(output, 'small-promo-tile.png') });
  await tile.close();
} finally {
  await browser.close();
}
