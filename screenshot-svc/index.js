const { chromium } = require('playwright');
const http = require('http');

const PORT = process.env.PORT || 3001;
let browser;

async function init() {
  browser = await chromium.launch({ args: ['--no-sandbox'] });
  console.log(`Screenshot service ready on port ${PORT}`);
}

const server = http.createServer(async (req, res) => {
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
      res.writeHead(400);
      res.end(JSON.stringify({ error: 'invalid JSON' }));
      return;
    }

    if (!url) {
      res.writeHead(400);
      res.end(JSON.stringify({ error: 'url required' }));
      return;
    }

    // SSRF guard: only allow public HTTP/HTTPS URLs
    let parsed;
    try { parsed = new URL(url); } catch {
      res.writeHead(400);
      res.end(JSON.stringify({ error: 'invalid url' }));
      return;
    }
    if (!['http:', 'https:'].includes(parsed.protocol)) {
      res.writeHead(400);
      res.end(JSON.stringify({ error: 'only http/https allowed' }));
      return;
    }
    const h = parsed.hostname.toLowerCase();
    const isPrivate =
      h === 'localhost' || h.endsWith('.local') ||
      /^127\./.test(h) || /^10\./.test(h) ||
      /^172\.(1[6-9]|2\d|3[01])\./.test(h) ||
      /^192\.168\./.test(h) || /^::1$/.test(h);
    if (isPrivate) {
      res.writeHead(403);
      res.end(JSON.stringify({ error: 'private urls not allowed' }));
      return;
    }

    let page;
    try {
      page = await browser.newPage();
      await page.setViewportSize({ width: 1200, height: 630 });
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
      // Brief settle for JS-rendered content after DOM is ready
      await new Promise(r => setTimeout(r, 2000));
      const screenshot = await page.screenshot({ type: 'jpeg', quality: 85 });
      res.writeHead(200, { 'Content-Type': 'image/jpeg' });
      res.end(screenshot);
    } catch (err) {
      console.error(`Screenshot failed for ${url}:`, err.message);
      res.writeHead(500);
      res.end(JSON.stringify({ error: err.message }));
    } finally {
      if (page) await page.close().catch(() => {});
    }
  });
});

init().then(() => server.listen(PORT));
