// Shared by the test files; not a test file itself.
const net = require('net');

/** A resolver for tests: each name maps to fixed addresses; unknown names do not resolve. */
function stubLookup(table) {
  return async host => (table[host] || []).map(address => ({ address, family: net.isIPv6(address) ? 6 : 4 }));
}

/** Listen on a free IPv4 loopback port and return it. */
async function listen(server) {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  return server.address().port;
}

module.exports = { listen, stubLookup };
