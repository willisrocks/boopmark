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

    let page;
    try {
      page = await browser.newPage();
      await page.setViewportSize({ width: 1200, height: 630 });
      await page.goto(url, { waitUntil: 'networkidle', timeout: 15000 });
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
