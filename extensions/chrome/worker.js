import "./core.js";
import "./api.js";

const Core = globalThis.BoopmarkCore;
const Api = globalThis.BoopmarkApi;
const SETTINGS_KEY = "boopmark.settings";
const OPERATION_KEY = "boopmark.operation";
const DRAFT_PREFIX = "boopmark-draft:";
const activeSaves = new Set();
const activeSuggestions = new Map();
const autoSuggestionTimers = new Map();

function invoke(owner, methodName, args = []) {
  const method = owner && owner[methodName];
  if (typeof method !== "function") return Promise.reject(new Error(`Chrome API ${methodName} is unavailable.`));
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (callback, value) => {
      if (!settled) { settled = true; callback(value); }
    };
    const callback = value => {
      const error = chrome.runtime.lastError;
      if (error) finish(reject, new Error(error.message || "Chrome API request failed."));
      else finish(resolve, value);
    };
    try {
      const result = method.call(owner, ...args, callback);
      if (result?.then) result.then(value => finish(resolve, value), error => finish(reject, error));
    } catch (error) { finish(reject, error); }
  });
}

const localArea = () => chrome.storage.local;
const sessionArea = () => chrome.storage.session;
const read = async (area, key) => (await invoke(area, "get", [key])) || {};
const write = (area, value) => invoke(area, "set", [value]);
const remove = (area, keys) => invoke(area, "remove", [Array.isArray(keys) ? keys : [keys]]);

// Restrict credentials and drafts before processing any command. Unsupported
// or failing access controls fail closed, including in the test harness.
const storageReady = Promise.all([
  invoke(localArea(), "setAccessLevel", [{ accessLevel: "TRUSTED_CONTEXTS" }]),
  invoke(sessionArea(), "setAccessLevel", [{ accessLevel: "TRUSTED_CONTEXTS" }]),
]);
storageReady.catch(() => {});
let transitions = Promise.resolve();
function serialize(action) {
  const result = transitions.then(async () => { await storageReady; return action(); });
  transitions = result.catch(() => {});
  return result;
}

function safeSettings(raw) {
  if (!raw || typeof raw !== "object") return null;
  const server = Core.normalizeServer(raw.server);
  const apiKey = typeof raw.apiKey === "string" ? raw.apiKey.trim() : "";
  return server && apiKey ? {
    server, apiKey, epoch: String(raw.epoch || "legacy"), authError: raw.authError === true,
  } : null;
}
async function getSettings() { return safeSettings((await read(localArea(), SETTINGS_KEY))[SETTINGS_KEY]); }
const putSettings = settings => write(localArea(), { [SETTINGS_KEY]: settings });
const putOperation = marker => write(localArea(), { [OPERATION_KEY]: marker });
const clearOperation = () => remove(localArea(), OPERATION_KEY);
async function getOperation() {
  let marker = (await read(localArea(), OPERATION_KEY))[OPERATION_KEY] || null;
  if (marker?.state === "pending" && !activeSaves.has(marker.id)) {
    marker = Core.recoverOperation(marker, false);
    await putOperation(marker);
  }
  return marker;
}
async function getDraft(id) { return id ? Core.normalizeDraft((await read(sessionArea(), id))[id]) : null; }
async function putDraft(draft) { await write(sessionArea(), { [draft.id]: draft }); return draft; }
async function clearDraft(id) { if (id) await remove(sessionArea(), id); }
async function clearAllDrafts() {
  const keys = Object.keys(await read(sessionArea(), null)).filter(key => key.startsWith(DRAFT_PREFIX));
  if (keys.length) await remove(sessionArea(), keys);
}
function safeError(error) {
  return { kind: String(error?.kind || "error"), message: String(error?.message || "Boopmark request failed."), status: Number(error?.status) || 0 };
}
function state(settings, draft, operation = null) {
  return {
    connection: {
      server: settings?.server || "", connected: Boolean(settings && !settings.authError), epoch: settings?.epoch || "",
      error: settings?.authError ? { kind: "auth", message: "Reconnect Boopmark." } : null,
    },
    draft: draft ? Core.normalizeDraft(draft) : null,
    operation,
  };
}
function publish() {
  try { chrome.runtime.sendMessage({ type: "STATE_CHANGED" })?.catch(() => {}); }
  catch { /* The popup may have closed; storage is authoritative. */ }
}
async function hasPermission(server) {
  try { return Boolean(await invoke(chrome.permissions, "contains", [{ origins: [Core.permissionPattern(server)] }])); }
  catch { return false; }
}
async function removePermission(server) {
  if (server) await invoke(chrome.permissions, "remove", [{ origins: [Core.permissionPattern(server)] }]);
}
async function removePermissionVerified(server) {
  if (!server) return;
  await removePermission(server);
  const stillGranted = await invoke(chrome.permissions, "contains", [{ origins: [Core.permissionPattern(server)] }]);
  if (stillGranted) throw new Api.ApiError("permission", "Server access could not be removed. Retry connection change.");
}
async function requireConnection() {
  const settings = await getSettings();
  if (!settings || settings.authError) throw new Api.ApiError("auth", "Reconnect Boopmark.");
  if (!(await hasPermission(settings.server))) throw new Api.ApiError("permission", "Reconnect Boopmark to grant server access.");
  return settings;
}
function sameConnection(settings, token) {
  return Boolean(settings && token && token.server === settings.server && token.connectionEpoch === settings.epoch);
}
function stopSuggestions(draftId) {
  for (const [id, timer] of autoSuggestionTimers) {
    if (!draftId || id === draftId) { clearTimeout(timer); autoSuggestionTimers.delete(id); }
  }
  for (const [key, request] of activeSuggestions) {
    if (!draftId || request.draftId === draftId) { request.controller.abort(); activeSuggestions.delete(key); }
  }
}
function suggestionKey(draftId, token) { return JSON.stringify([draftId, token.connectionEpoch, token.generation]); }

// Network work is detached. Only its guarded result merge enters the state
// queue, so edits, cancellation, and save dispatch stay responsive.
function startSuggestion(settings, draft, token) {
  const key = suggestionKey(draft.id, token);
  if (activeSuggestions.has(key)) return;
  const controller = new AbortController();
  activeSuggestions.set(key, { draftId: draft.id, controller });
  const client = new Api.BoopmarkApiClient({ server: settings.server, apiKey: settings.apiKey });
  (async () => {
    let suggestion;
    let failure;
    try { suggestion = await client.suggest(token.url, { signal: controller.signal }); }
    catch (error) { failure = error; }
    await serialize(async () => {
      // Cancel/reopen can recreate the same draft ID and generation. Only
      // this live request instance may merge into it, even if abort raced.
      if (activeSuggestions.get(key)?.controller !== controller) return;
      const currentSettings = await getSettings();
      if (!sameConnection(currentSettings, token)) return;
      const current = await getDraft(draft.id);
      if (!current || !Core.isCurrentRequest(current, token) || current.status === "saving") return;
      if (failure?.kind === "auth") { currentSettings.authError = true; await putSettings(currentSettings); }
      const result = failure ? Core.completeSuggestionError(current, token, failure.message) : Core.applySuggestion(current, suggestion, token);
      if (!result.stale) await putDraft(result.draft);
      publish();
    });
  })().catch(() => {}).finally(() => {
    if (activeSuggestions.get(key)?.controller === controller) activeSuggestions.delete(key);
  });
}
async function beginSuggestion(settings, draft) {
  const begun = Core.beginSuggestion(draft);
  await putDraft(begun.draft);
  startSuggestion(settings, begun.draft, begun.token);
  return begun.draft;
}
function queueAutoSuggestion(draftId) {
  if (autoSuggestionTimers.has(draftId)) clearTimeout(autoSuggestionTimers.get(draftId));
  autoSuggestionTimers.set(draftId, setTimeout(() => {
    autoSuggestionTimers.delete(draftId);
    serialize(async () => {
      const settings = await requireConnection();
      const draft = await getDraft(draftId);
      if (!draft || draft.suggestionAttempted || !Core.validBookmarkURL(draft.url) || await getOperation()) return;
      await beginSuggestion(settings, draft);
      publish();
    }).catch(() => {});
  }, 350));
}

async function connect(message) {
  const server = Core.normalizeServer(message?.server);
  const apiKey = typeof message?.apiKey === "string" ? message.apiKey.trim() : "";
  if (!server || !apiKey) throw new Api.ApiError("configuration", "Enter an HTTPS server origin and a Boopmark API key.");
  const previous = await getSettings();
  try {
    const operation = await getOperation();
    if (operation && !(operation.state === "error" && previous?.server === server)) {
      throw new Api.ApiError("state", "Check and dismiss the recorded save result before changing connections, or disconnect explicitly.");
    }
    if (!(await hasPermission(server))) throw new Api.ApiError("permission", "Grant server access using Connect.");
    await new Api.BoopmarkApiClient({ server, apiKey }).validateConnection();
    if (previous?.server && previous.server !== server) {
      await removePermissionVerified(previous.server);
      await clearAllDrafts();
    }
    stopSuggestions();
    const settings = { server, apiKey, epoch: Core.randomId(), authError: false };
    await putSettings(settings);
    publish();
    return state(settings, null);
  } catch (error) {
    if (server !== previous?.server) await removePermission(server).catch(() => {});
    throw error;
  }
}

async function open(message) {
  let settings = await getSettings();
  const operation = await getOperation();
  if (!settings) return state(null, null, operation);
  if (!(await hasPermission(settings.server))) settings = { ...settings, authError: true };

  // The single operation record takes precedence across tabs and restarts.
  // Its draft ID, not the current URL, identifies an edited-URL submission.
  if (operation) {
    let draft = await getDraft(operation.draftId);
    if (!draft) {
      draft = Core.createDraft({ id: operation.draftId, server: operation.server,
        connectionEpoch: operation.connectionEpoch, url: operation.url, title: "" });
      draft.suggestionAttempted = true;
      if (operation.state !== "success") await putDraft(draft);
    }
    if (draft.connectionEpoch !== settings.epoch && operation.state === "error") {
      draft = Core.rebindDraft(draft, settings.epoch, settings.server);
      draft.suggestionAttempted = true;
      await putDraft(draft);
    }
    return state(settings, draft, operation);
  }

  const tab = message?.tab || {};
  const capture = {
    server: settings.server, connectionEpoch: settings.epoch,
    tabId: Number.isInteger(tab.id) ? tab.id : -1,
    url: typeof tab.url === "string" ? tab.url : "", title: typeof tab.title === "string" ? tab.title : "",
  };
  const id = Core.makeDraftId(capture);
  let draft = await getDraft(id);
  if (!draft) draft = await putDraft(Core.createDraft({ ...capture, id }));
  else if (draft.connectionEpoch !== settings.epoch) draft = await putDraft(Core.rebindDraft(draft, settings.epoch, settings.server));
  if (!draft.suggestionAttempted && Core.validBookmarkURL(draft.url) && !settings.authError && !autoSuggestionTimers.has(id)) {
    draft = await beginSuggestion(settings, draft);
  } else if (draft.metadataStatus === "loading" && !activeSuggestions.has(suggestionKey(id, Core.requestToken(draft)))) {
    draft.metadataStatus = "error";
    draft.error = "Metadata lookup was interrupted. Retry or save as-is.";
    draft.revision += 1;
    await putDraft(draft);
  }
  return state(settings, draft);
}

async function update(message) {
  // Local edits queued just before an auth/permission failure must survive
  // the transition to reconnect. Only network actions require a live grant.
  const settings = await getSettings();
  const draft = await getDraft(message?.draftId);
  if (!draft || !sameConnection(settings, draft)) throw new Api.ApiError("state", "This bookmark draft is no longer available.");
  const operation = await getOperation();
  if (operation && (operation.state !== "error" || operation.draftId !== draft.id)) throw new Api.ApiError("state", "Check the recorded save result before editing.");
  if (!["url", "title", "description", "tags"].includes(message?.field)) throw new Api.ApiError("validation", "Unknown bookmark field.");
  const next = Core.updateField(draft, message.field, message.value);
  await putDraft(next);
  if (message.field === "url") {
    stopSuggestions(next.id);
    if (!operation && !settings.authError && Core.validBookmarkURL(next.url)) queueAutoSuggestion(next.id);
  }
  return state(settings, next, operation);
}
async function suggest(message) {
  const settings = await requireConnection();
  const draft = await getDraft(message?.draftId);
  if (!draft || !sameConnection(settings, draft)) throw new Api.ApiError("state", "This bookmark draft is no longer available.");
  if (await getOperation()) throw new Api.ApiError("state", "Dismiss the recorded save result before fetching metadata.");
  if (!Core.validBookmarkURL(draft.url)) throw new Api.ApiError("validation", "Open a web page to add a bookmark.");
  if (activeSuggestions.has(suggestionKey(draft.id, Core.requestToken(draft)))) return state(settings, draft);
  stopSuggestions(draft.id);
  return state(settings, await beginSuggestion(settings, draft));
}

function executeSave(settings, marker, fields) {
  (async () => {
    let bookmark;
    let failure;
    try {
      bookmark = await new Api.BoopmarkApiClient({ server: settings.server, apiKey: settings.apiKey })
        .create(fields, { idempotencyKey: marker.id });
    }
    catch (error) { failure = error; }
    await serialize(async () => {
      const currentSettings = await getSettings();
      const currentMarker = await getOperation();
      if (!sameConnection(currentSettings, marker) || currentMarker?.id !== marker.id) return;
      if (failure?.kind === "auth") { currentSettings.authError = true; await putSettings(currentSettings); }
      const outcome = !failure ? { state: "success", bookmarkId: bookmark.id }
        : { state: failure.kind === "unknown" ? "unknown" : "error", error: failure.message };
      await putOperation(Core.transitionOperation(marker, outcome));
      if (!failure) await clearDraft(marker.draftId);
      else {
        const draft = await getDraft(marker.draftId);
        if (draft) { draft.status = "ready"; draft.revision += 1; await putDraft(draft); }
      }
      publish();
    });
  })().catch(() => publish()).finally(() => activeSaves.delete(marker.id));
}
async function save(message) {
  const settings = await getSettings();
  const draft = await getDraft(message?.draftId);
  if (!draft || !sameConnection(settings, draft)) throw new Api.ApiError("state", "This bookmark draft is no longer available.");
  const operation = await getOperation();
  if (operation && !(operation.state === "error" && operation.draftId === draft.id)) return state(settings, draft, operation);
  const fields = Core.serializeFields(message?.fields);
  if (!Core.validBookmarkURL(fields.url)) throw new Api.ApiError("validation", "Open a web page to add a bookmark.");
  // The popup sends its snapshot immediately, superseding its queued UPDATEs.
  // Preserve it locally even if permission/auth fails before network dispatch.
  const reviewed = { ...draft, ...fields, tags: Core.formatTags(fields.tags),
    dirty: { url: true, title: true, description: true, tags: true },
    generated: { title: false, description: false, tags: false }, status: "ready", suggestionAttempted: true,
    metadataStatus: draft.metadataStatus === "loading" ? "empty" : draft.metadataStatus,
    generation: draft.generation + 1, revision: draft.revision + 1 };
  await putDraft(reviewed);
  stopSuggestions(draft.id);
  await requireConnection();
  const marker = Core.operationMarker(fields, { server: settings.server, connectionEpoch: settings.epoch, draftId: draft.id });
  // Register before persistence; no network request occurs before both writes.
  activeSaves.add(marker.id);
  try {
    await putOperation(marker);
    const frozen = { ...reviewed, status: "saving", revision: reviewed.revision + 1 };
    await putDraft(frozen);
    executeSave(settings, marker, fields);
    return state(settings, frozen, marker);
  } catch (error) { activeSaves.delete(marker.id); throw error; }
}
async function cancel(message) {
  const operation = await getOperation();
  if (operation) return state(await getSettings(), await getDraft(operation.draftId), operation);
  stopSuggestions(message?.draftId);
  await clearDraft(message?.draftId);
  publish();
  return state(await getSettings(), null);
}
async function acknowledge(message) {
  const operation = await getOperation();
  if (!operation || operation.id !== message?.operationId || operation.state === "pending") return state(await getSettings(), await getDraft(operation?.draftId), operation);
  await clearOperation();
  publish();
  return state(await getSettings(), await getDraft(operation.draftId));
}
async function disconnect() {
  const settings = await getSettings();
  stopSuggestions();
  if (settings?.server) {
    // Keep the identity and operation recoverable until host access is
    // verifiably gone. Do not use hasPermission: its fail-closed fallback
    // cannot distinguish a failed verification from an absent grant.
    await removePermission(settings.server);
    const stillGranted = await invoke(chrome.permissions, "contains", [{ origins: [Core.permissionPattern(settings.server)] }]);
    if (stillGranted) throw new Api.ApiError("permission", "Server access could not be removed. Retry Disconnect.");
  }
  await clearAllDrafts();
  await clearOperation();
  await putSettings(null);
  publish();
  return state(null, null);
}
async function handleMessage(message) {
  switch (message?.type) {
    case "OPEN": return open(message);
    case "UPDATE": return update(message);
    case "SUGGEST": return suggest(message);
    case "SAVE": return save(message);
    case "CANCEL": return cancel(message);
    case "CONNECT": return connect(message);
    case "DISCONNECT": return disconnect();
    case "ACK": return acknowledge(message);
    default: throw new Api.ApiError("protocol", "Unknown Boopmark message.");
  }
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const extensionRoot = chrome.runtime.getURL("");
  if (sender.id !== chrome.runtime.id || !sender.url?.startsWith(extensionRoot)) return false;
  serialize(() => handleMessage(message))
    .then(result => sendResponse({ ok: true, ...result }))
    .catch(error => sendResponse({ ok: false, error: safeError(error) }));
  return true;
});
chrome.runtime.onStartup?.addListener(() => serialize(getOperation).catch(() => {}));
chrome.runtime.onInstalled?.addListener(() => serialize(getOperation).catch(() => {}));
