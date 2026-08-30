import './core.js';
const permissionPattern = globalThis.BoopmarkCore.permissionPattern;
const $ = id => document.getElementById(id);
const fields = ['url', 'title', 'description', 'tags'];
let capturedTab;
let state = {};
let settingsVisible = false;
let connecting = false;
let saving = false;
let updates = Promise.resolve();
let writeCount = 0;
let closeTimer;
let refreshVersion = 0;
let editVersion = 0;
let renderedEditVersion = -1;
let displayedServer;
let closing = false;
let submissionVersion = 0;

function updateSettingsLink() {
  try { $('settings-link').href = `${serverOrigin($('server').value)}/settings`; }
  catch { $('settings-link').removeAttribute('href'); }
}

function validURL(value) {
  if (/[\u0000-\u001f\u007f]/.test(value)) return false;
  try { const url = new URL(value); return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password; }
  catch { return false; }
}
function serverOrigin(value) {
  const url = new URL(value.trim());
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if ((url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) || url.username || url.password || url.search || url.hash || !['', '/'].includes(url.pathname)) {
    throw new Error('Use an HTTPS server origin, or HTTP localhost for development.');
  }
  return url.origin;
}
async function send(message) {
  const result = await chrome.runtime.sendMessage(message);
  if (!result) throw new Error('Boopmark did not respond. Reopen the popup to recover your draft.');
  if (result.ok === false) throw Object.assign(new Error(result.error?.message || result.error || 'Request failed.'), { kind: result.error?.kind });
  return result;
}
function displayError(id, error) {
  $(id).textContent = error.message || String(error);
  $(id).classList.add('error');
  $(id).hidden = false;
}
function formValues() { return Object.fromEntries(fields.map(field => [field, $(field).value])); }
function validateForm() {
  const valid = validURL($('url').value);
  $('url-error').hidden = valid;
  $('url-error').textContent = valid ? '' : 'Open a web page to add a bookmark, or enter a valid HTTP(S) URL without embedded credentials.';
  $('save-button').disabled = !valid || saving || Boolean(state.operation && ['pending', 'unknown', 'success'].includes(state.operation.state)) || !state.connection?.connected;
  $('autofill-button').disabled = !valid || saving || Boolean(state.operation) || state.draft?.metadataStatus === 'loading' || !state.connection?.connected;
}
function render() {
  const connected = Boolean(state.connection?.connected);
  const setup = settingsVisible || (!connected && !state.operation);
  $('setup-heading').textContent = state.connection?.server && !connected ? 'Reconnect Boopmark' : 'Connect Boopmark';
  $('setup').hidden = !setup;
  $('capture').hidden = setup;
  $('settings-button').hidden = setup;
  $('settings-button').textContent = connected ? 'Settings' : 'Reconnect';
  $('back-button').hidden = !connected && !state.operation;
  $('disconnect-button').hidden = !state.connection?.server;
  if (state.connection?.server && displayedServer !== state.connection.server && !connecting) {
    $('server').value = state.connection.server;
    displayedServer = state.connection.server;
  }
  updateSettingsLink();
  if (state.connection?.server) $('library-link').href = `${state.connection.server}/bookmarks`;
  const draft = state.draft;
  if (draft && !writeCount && !saving && renderedEditVersion === editVersion) {
    for (const field of fields) $('' + field).value = Array.isArray(draft[field]) ? draft[field].join(', ') : draft[field] ?? '';
  }
  const operation = state.operation;
  const locked = saving || Boolean(operation && ['pending', 'unknown', 'success'].includes(operation.state));
  for (const field of fields) $(field).disabled = locked;
  $('save-button').textContent = operation?.state === 'pending' || saving ? 'Saving…' : 'Add Bookmark';
  $('cancel-button').textContent = operation ? 'Close' : 'Cancel';
  $('close-button').setAttribute('aria-label', operation ? 'Close bookmark popup' : 'Cancel and discard bookmark');
  const metadata = {
    loading: 'Fetching metadata…', filled: 'Metadata filled. Review before saving.',
    empty: 'No metadata available. Edit fields or save as-is.',
    error: draft?.error?.message || draft?.error || 'Could not fetch metadata. Retry or save as-is.',
  };
  $('metadata-status').textContent = !connected && state.operation
    ? 'Reconnect in Settings to continue. This recorded save will not be sent again.'
    : metadata[draft?.metadataStatus] || 'Review the page details before saving.';
  $('metadata-status').classList.toggle('loading', draft?.metadataStatus === 'loading');
  $('save-status').className = operation?.state === 'success' ? 'success' : operation?.state === 'pending' ? '' : 'error';
  $('save-status').textContent = operation?.state === 'success' ? 'Saved to Boopmark' : operation?.state === 'pending' ? 'Saving… You can close this popup safely.' : operation?.state === 'unknown' ? 'Save could not be confirmed. Check Boopmark before retrying. Nothing will be resent automatically.' : operation?.error?.message || operation?.error || '';
  $('recovery').hidden = !operation || !['unknown', 'error'].includes(operation.state);
  $('library-link').hidden = operation?.state !== 'unknown';
  $('ack-button').textContent = operation?.state === 'error' ? 'Dismiss this failed save' : 'I checked; dismiss this result';
  if (operation?.state !== 'success' && closeTimer) {
    clearTimeout(closeTimer);
    closeTimer = undefined;
  }
  if (operation?.state === 'success' && !closeTimer) {
    closeTimer = setTimeout(async () => {
      closing = true;
      try { await send({ type: 'ACK', operationId: operation.id }); window.close(); }
      catch (error) { closing = false; displayError('save-status', error); }
    }, 1300);
  }
  validateForm();
}
async function refresh() {
  if (closing) return;
  const version = ++refreshVersion;
  await updates;
  if (closing) return;
  const revision = editVersion;
  const result = await send({ type: 'OPEN', tab: capturedTab });
  if (closing || version !== refreshVersion || revision !== editVersion) return;
  state = result;
  renderedEditVersion = revision;
  render();
}
for (const field of fields) {
  $(field).addEventListener('input', () => {
    editVersion++;
    validateForm();
    const draftId = state.draft?.id;
    const value = $(field).value;
    const submission = submissionVersion;
    if (!draftId) return;
    writeCount++;
    updates = updates.then(() => submission === submissionVersion ? send({ type: 'UPDATE', draftId, field, value }) : undefined)
      .catch(error => displayError('save-status', error))
      .finally(() => {
        writeCount--;
        if (!writeCount) refresh().catch(error => displayError('save-status', error));
      });
  });
}
$('bookmark-form').addEventListener('submit', async event => {
  event.preventDefault();
  if ($('save-button').disabled) return;
  const snapshot = formValues();
  submissionVersion++;
  let dispatchError;
  saving = true;
  render();
  try {
    // Dispatch while the click handler is still alive. The visible snapshot
    // is authoritative even if earlier field-persistence messages are pending;
    // waiting for those would let popup closure silently prevent Save.
    await send({ type: 'SAVE', draftId: state.draft?.id, fields: snapshot });
  } catch (error) { dispatchError = error; }
  finally {
    saving = false;
    await refresh().catch(error => displayError('save-status', error));
    if (dispatchError && !state.operation) displayError('save-status', dispatchError);
  }
});
$('autofill-button').addEventListener('click', async () => {
  try { await updates; await send({ type: 'SUGGEST', draftId: state.draft?.id }); await refresh(); }
  catch (error) { displayError('save-status', error); }
});
async function dismiss() {
  closing = true;
  if (saving || state.operation) { window.close(); return; }
  // Enqueue the discard now; closing the popup must not lose a Cancel click
  // while older field writes are pending. The worker orders received writes.
  submissionVersion++;
  try { await send({ type: 'CANCEL', draftId: state.draft?.id }); window.close(); }
  catch (error) { closing = false; displayError('save-status', error); }
}
$('cancel-button').addEventListener('click', dismiss);
$('close-button').addEventListener('click', dismiss);
$('settings-button').addEventListener('click', () => { settingsVisible = true; render(); $('server').focus(); });
$('back-button').addEventListener('click', () => { settingsVisible = false; render(); $('url').focus(); });
$('server').addEventListener('input', updateSettingsLink);
$('connection-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (connecting) return;
  let server;
  try { server = serverOrigin($('server').value); }
  catch (error) { displayError('connection-status', error); return; }
  const apiKey = $('api-key').value.trim();
  const formerServer = state.connection?.server;
  let granted = false;
  let connected = false;
  connecting = true;
  $('connect-button').disabled = true;
  for (const id of ['server', 'api-key', 'disconnect-button', 'back-button']) $(id).disabled = true;
  $('connection-status').classList.remove('error');
  $('connection-status').textContent = 'Connecting…';
  try {
    // Keep permission request directly within the Connect user gesture. It
    // can throw synchronously in a restricted/invalid Chrome context, so it
    // must remain inside this handler's recovery path as well.
    const grant = chrome.permissions.request({ origins: [permissionPattern(server)] });
    granted = await grant;
    if (!granted) throw new Error('Server access was not granted. Click Connect to try again.');
    await send({ type: 'CONNECT', server, apiKey });
    connected = true;
    $('api-key').value = '';
    $('connection-status').textContent = '';
    settingsVisible = false;
    await refresh();
  } catch (error) {
    // Also roll back if the worker never received CONNECT. Never revoke the
    // existing connection's access during an unsuccessful replacement.
    if (granted && !connected && server !== formerServer) {
      await chrome.permissions.remove({ origins: [permissionPattern(server)] }).catch(() => {});
    }
    displayError('connection-status', error);
  }
  finally {
    connecting = false;
    $('connect-button').disabled = false;
    for (const id of ['server', 'api-key', 'disconnect-button', 'back-button']) $(id).disabled = false;
  }
});
$('disconnect-button').addEventListener('click', async () => {
  try {
    await updates;
    await send({ type: 'DISCONNECT' });
    $('api-key').value = '';
    settingsVisible = true;
    await refresh();
  } catch (error) { displayError('connection-status', error); }
});
$('ack-button').addEventListener('click', async () => {
  try { await send({ type: 'ACK', operationId: state.operation?.id }); await refresh(); }
  catch (error) { displayError('save-status', error); }
});
chrome.runtime.onMessage.addListener(message => {
  if (!closing && message.type === 'STATE_CHANGED') refresh().catch(error => displayError('fatal-error', error));
});
try {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  capturedTab = { id: tab?.id ?? -1, url: tab?.url || '', title: tab?.title || '' };
  await refresh();
} catch (error) { displayError('fatal-error', error); }
