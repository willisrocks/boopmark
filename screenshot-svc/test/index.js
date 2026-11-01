// `node --test screenshot-svc/test` runs this directory as a module on
// Node 22+, so load every test file from here.
const fs = require('fs');
const path = require('path');

for (const file of fs.readdirSync(__dirname).filter(f => f.endsWith('.test.js')).sort()) {
  require(path.join(__dirname, file));
}
