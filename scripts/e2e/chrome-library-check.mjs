// Read-only verification of one real library card in a pinned regular tab.
// Credentials stay in memory; no broad auth headers, cookies, HAR, or raw logs.
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const emit = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const fail = () => { throw new Error('Library verification unavailable.'); };

export function parseOptions(args, env = process.env) {
  if (args.length < 5 || args.length > 6) fail();
  const [mode, keyFile, bookmarkId, fixture, title, screenshot] = args;
  if (mode !== '--key-file' || !/^\/private\/tmp\/boopmark-extension-qa\.[A-Za-z0-9]+\/boopmark-api-key$/.test(keyFile)) fail();
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(bookmarkId)) fail();
  const url = new URL(fixture);
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || /[\u0000-\u0020\u007f]/.test(fixture) || !title || title.length > 500) fail();
  const targetId = env.CHROME_LIBRARY_TARGET_ID;
  if (!/^[A-Fa-f0-9]{32}$/.test(targetId || '')) fail();
  const screenshotPath = screenshot ? path.resolve(root, screenshot) : null;
  if (screenshotPath && (!screenshotPath.startsWith(`${root}/test-results/`) || !screenshotPath.endsWith('.png'))) fail();
  return { keyFile, bookmarkId, fixture, title, targetId, screenshotPath, libraryURL: `https://boopmark.com/bookmarks?search=${encodeURIComponent(fixture)}` };
}

export function shouldAuthenticate(request, resourceType, frameId, expectedFrameId, libraryURL) {
  return resourceType === 'Document' && frameId === expectedFrameId && request?.method === 'GET' && request.url === libraryURL;
}

async function findTab(targetId) {
  const result = spawnSync(process.execPath, [path.join(root, 'scripts/e2e/chrome-browser.mjs'), 'get', 'cdp-url'], {
    encoding: 'utf8', timeout: 10_000, maxBuffer: 262_144, stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.status !== 0) fail();
  const endpoints = result.stdout.split('\n').map(line => line.trim()).filter(line => line.startsWith('ws://127.0.0.1:'));
  if (endpoints.length !== 1) fail();
  const endpoint = new URL(endpoints[0]);
  if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1' || !endpoint.port || endpoint.username || endpoint.password || endpoint.search || endpoint.hash || !endpoint.pathname.startsWith('/devtools/browser/')) fail();
  const response = await fetch(`http://${endpoint.host}/json/list`, { redirect: 'error', signal: AbortSignal.timeout(5_000) });
  if (!response.ok) fail();
  const targets = await response.json();
  const matches = Array.isArray(targets) ? targets.filter(target => target.id === targetId && target.type === 'page' && /^(https?:|chrome-error:|about:blank$)/.test(target.url)) : [];
  if (matches.length !== 1) fail();
  const socketURL = new URL(matches[0].webSocketDebuggerUrl);
  if (socketURL.origin !== endpoint.origin || socketURL.pathname !== `/devtools/page/${targetId}` || socketURL.search || socketURL.hash || socketURL.username || socketURL.password) fail();
  return socketURL.href;
}

async function check(options) {
  const socket = new WebSocket(await findTab(options.targetId));
  let credential = '';
  let nextId = 0;
  let frameId;
  let interceptionFailed = false;
  let authenticatedRequests = 0;
  let documentNavigations = 0;
  const pending = new Map();
  function command(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error('Command unavailable.')); }, 8_000);
      pending.set(id, { resolve, reject, timer });
      try { socket.send(JSON.stringify({ id, method, params })); }
      catch { clearTimeout(timer); pending.delete(id); reject(new Error('Command unavailable.')); }
    });
  }
  socket.addEventListener('message', event => {
    // Protocol payloads, including headers, never leave this process.
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message.id) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id); clearTimeout(waiter.timer);
      if (message.error) waiter.reject(new Error('Command unavailable.'));
      else waiter.resolve(message.result);
    } else if (message.method === 'Page.frameNavigated') {
      if (message.params.frame.id === frameId && message.params.frame.url === options.libraryURL) documentNavigations++;
    } else if (message.method === 'Fetch.requestPaused') {
      const params = message.params;
      const override = { requestId: params.requestId };
      if (credential && shouldAuthenticate(params.request, params.resourceType, params.frameId, frameId, options.libraryURL)) {
        override.headers = Object.entries(params.request.headers).filter(([name]) => name.toLowerCase() !== 'authorization').map(([name, value]) => ({ name, value: String(value) }));
        override.headers.push({ name: 'Authorization', value: `Bearer ${credential}` });
        authenticatedRequests++;
      }
      // Fetch header overrides apply only to this request, not redirect hops.
      command('Fetch.continueRequest', override).catch(() => { interceptionFailed = true; });
    }
  });
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Debugger unavailable.')), 5_000);
      socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('Debugger unavailable.')); }, { once: true });
    });
    const info = (await command('Target.getTargetInfo')).targetInfo;
    if (info.targetId !== options.targetId || info.type !== 'page' || !/^(https?:|chrome-error:|about:blank$)/.test(info.url)) fail();
    await command('Page.enable');
    frameId = (await command('Page.getFrameTree')).frameTree.frame.id;
    // User-authorized cache only: private, owned, regular file in private dir.
    const keyStat = lstatSync(options.keyFile), dirStat = lstatSync(path.dirname(options.keyFile));
    if (!keyStat.isFile() || !dirStat.isDirectory() || keyStat.uid !== process.getuid() || dirStat.uid !== process.getuid()
      || (keyStat.mode & 0o777) !== 0o600 || (dirStat.mode & 0o777) !== 0o700 || keyStat.size < 1 || keyStat.size > 16_384) fail();
    credential = readFileSync(options.keyFile, 'utf8').trim();
    if (!credential || /[\r\n]/.test(credential)) fail();
    await command('Fetch.enable', { patterns: [{ urlPattern: 'https://boopmark.com/bookmarks*', resourceType: 'Document', requestStage: 'Request' }] });
    const expression = `(() => {
      if (location.href !== ${JSON.stringify(options.libraryURL)} || document.readyState !== 'complete') return null;
      const cards = document.querySelectorAll(${JSON.stringify(`#bookmark-${options.bookmarkId}`)});
      if (cards.length !== 1) return null;
      const card = cards[0], content = card.querySelector(':scope > div.p-4');
      if (!content) return null;
      const title = content.querySelector(':scope > a');
      card.scrollIntoView({block:'center'});
      const rect = card.getBoundingClientRect();
      return { present:true, titleMatches:title?.textContent.trim() === ${JSON.stringify(options.title)},
        linkMatches:title?.getAttribute('href') === ${JSON.stringify(options.fixture)} && card.querySelector('[data-testid="bookmark-card-image-link"]')?.getAttribute('href') === ${JSON.stringify(options.fixture)},
        descriptionClear:!(content.querySelector(':scope > p')?.textContent.trim()),
        tags:[...content.querySelectorAll(':scope > div.flex.flex-wrap > span')].slice(0,30).map(tag => tag.textContent.trim().slice(0,100)),
        clip:{x:rect.left+scrollX,y:rect.top+scrollY,width:rect.width,height:rect.height,scale:1} };
    })()`;
    async function inspect() {
      for (let attempt = 0; attempt < 40; attempt++) {
        if (interceptionFailed) fail();
        const result = await command('Runtime.evaluate', { expression, returnByValue: true });
        if (result.result?.value) return result.result.value;
        await new Promise(resolve => setTimeout(resolve, 250));
      }
      fail();
    }
    if ((await command('Page.navigate', { url: options.libraryURL })).errorText) fail();
    await inspect();
    const firstNavigation = documentNavigations;
    await command('Page.reload', { ignoreCache: true });
    // Await a second authenticated document, not the previous page's DOM.
    for (let attempt = 0; (authenticatedRequests < 2 || documentNavigations <= firstNavigation) && attempt < 40; attempt++) await new Promise(resolve => setTimeout(resolve, 250));
    if (authenticatedRequests !== 2 || documentNavigations <= firstNavigation) fail();
    const evidence = await inspect();
    const passed = evidence.present && evidence.titleMatches && evidence.linkMatches && evidence.descriptionClear;
    if (options.screenshotPath && passed) {
      const clip = evidence.clip;
      if (![clip.x, clip.y, clip.width, clip.height].every(Number.isFinite) || clip.x < 0 || clip.y < 0 || clip.width < 1 || clip.height < 1 || clip.width > 2000 || clip.height > 2000) fail();
      mkdirSync(path.dirname(options.screenshotPath), { recursive: true, mode: 0o700 });
      if (!realpathSync(path.dirname(options.screenshotPath)).startsWith(`${root}/test-results/`) && realpathSync(path.dirname(options.screenshotPath)) !== `${root}/test-results`) fail();
      const screenshot = await command('Page.captureScreenshot', { format: 'png', clip, captureBeyondViewport: true });
      const afterCapture = await inspect();
      if (!afterCapture.titleMatches || !afterCapture.linkMatches || !afterCapture.descriptionClear || JSON.stringify(afterCapture.clip) !== JSON.stringify(clip)) fail();
      writeFileSync(options.screenshotPath, Buffer.from(screenshot.data, 'base64'), { mode: 0o600, flag: 'wx' });
    }
    delete evidence.clip;
    emit({ type: 'library_check', ...evidence, reloaded: true, authenticatedDocumentRequests: authenticatedRequests, screenshotPath: options.screenshotPath && passed ? options.screenshotPath : null });
    if (!passed) process.exitCode = 1;
  } finally {
    credential = '';
    if (socket.readyState === WebSocket.OPEN) await command('Fetch.disable').catch(() => {});
    for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error('Verifier ended.')); }
    pending.clear(); socket.close();
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try { await check(parseOptions(process.argv.slice(2))); }
  catch { emit({ type: 'library_check_error', message: 'Read-only library verification unavailable; no credentials or raw diagnostics were emitted.' }); process.exitCode = 1; }
}
