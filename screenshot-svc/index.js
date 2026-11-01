const http = require('http');
const { Blocked, InvalidUrl, createGuard } = require('./guard');
const { PROXY_ERROR_HEADER, startGuardProxy } = require('./proxy');

const PORT = process.env.PORT || 3001;

// SSRF guard, in two layers:
//
// 1. page.route('**/*') checks every request the page makes (the navigation
//    and its subresources) without DNS, and aborts refused ones early: bad
//    schemes, ports and credentials, local names and private literal IPs.
// 2. Chromium is forced through a loopback proxy (proxy.js) that checks
//    every request again, including redirect hops and service-worker
//    fetches that page.route never sees, and connects to the exact address it
//    checked. The browser never resolves a destination name itself, so a
//    name cannot be rebound to a private address between check and connect.
//
// Host names are resolved and judged in layer 2 only, the enforcement point.

/**
 * Launch Chromium with all traffic sent through a guard proxy.
 * `<-loopback>` removes Chromium's implicit proxy bypass for localhost, and
 * WebRTC may not use UDP outside the proxy.
 */
async function launchBrowser(guard, { executablePath } = {}) {
  const { chromium } = require('playwright');
  const proxy = await startGuardProxy(guard);
  try {
    const browser = await chromium.launch({
      executablePath,
      args: [
        '--no-sandbox',
        `--proxy-server=http://127.0.0.1:${proxy.port}`,
        '--proxy-bypass-list=<-loopback>',
        '--force-webrtc-ip-handling-policy=disable_non_proxied_udp',
      ],
    });
    return {
      browser,
      close: async () => {
        await browser.close();
        await proxy.close();
      },
    };
  } catch (err) {
    await proxy.close();
    throw err;
  }
}

/** Abort every request of `page` that the guard refuses without a lookup. */
async function guardPage(page, guard, log = console) {
  await page.route('**/*', async route => {
    const url = route.request().url();
    try {
      guard.check(url);
    } catch (err) {
      log.warn('Refused %s: %s', url, err.message);
      await route.abort('blockedbyclient');
      return;
    }
    await route.continue();
  });
}

function sendJson(res, status, body) {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(body));
}

/** The screenshot HTTP service, given a browser launched by `launchBrowser`. */
function createServer({ browser, guard, settleMs = 2000 }) {
  return http.createServer((req, res) => {
    if (req.method !== 'POST' || req.url !== '/screenshot') {
      res.writeHead(404);
      res.end();
      return;
    }

    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', async () => {
      let url;
      try {
        ({ url } = JSON.parse(body));
      } catch {
        sendJson(res, 400, { error: 'invalid JSON' });
        return;
      }

      if (!url) {
        sendJson(res, 400, { error: 'url required' });
        return;
      }

      try {
        await guard.checkUrl(url);
      } catch (err) {
        const status = err instanceof Blocked && !(err instanceof InvalidUrl) ? 403 : 400;
        sendJson(res, status, { error: err.message });
        return;
      }

      let page;
      try {
        page = await browser.newPage({ serviceWorkers: 'block', viewport: { width: 1200, height: 630 } });
        await guardPage(page, guard);
        const response = await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        // The proxy made this response up: a redirect led somewhere it
        // refused, or the fetch failed. Never capture its error text.
        const proxyError = response && response.headers()[PROXY_ERROR_HEADER];
        if (proxyError === 'blocked') {
          sendJson(res, 403, { error: 'private urls not allowed' });
          return;
        }
        if (proxyError) {
          sendJson(res, 500, { error: `fetch failed: ${(await response.text()).slice(0, 200)}` });
          return;
        }
        // Brief settle for JS-rendered content after DOM is ready
        await new Promise(r => setTimeout(r, settleMs));
        const screenshot = await page.screenshot({ type: 'jpeg', quality: 85 });
        res.writeHead(200, { 'Content-Type': 'image/jpeg' });
        res.end(screenshot);
      } catch (err) {
        console.error('Screenshot failed for %s: %s', url, err.message);
        sendJson(res, 500, { error: err.message });
      } finally {
        if (page) await page.close().catch(() => {});
      }
    });
  });
}

module.exports = { createServer, guardPage, launchBrowser };

if (require.main === module) {
  const guard = createGuard();
  launchBrowser(guard).then(({ browser }) => {
    createServer({ browser, guard }).listen(PORT);
    console.log(`Screenshot service ready on port ${PORT}`);
  });
}
