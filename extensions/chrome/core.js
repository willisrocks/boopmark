(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.BoopmarkCore = factory();
  }
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  // Keep these values in one place.  They are intentionally strings so the
  // popup can render them as-is and the API client can send explicit empty
  // values when a user intentionally clears a field.
  var FIELD_NAMES = ["title", "description", "tags"];
  var UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

  function isLoopbackHostname(hostname) {
    var host = String(hostname || "").toLowerCase().replace(/^\[|\]$/g, "");
    return host === "localhost" || host === "127.0.0.1" || host === "::1";
  }

  /**
   * Validate and canonicalise a Boopmark server URL.
   *
   * Hosted/self-hosted servers must use HTTPS.  HTTP is only accepted for
   * loopback development servers.  A server URL is an origin in the MVP, so
   * query strings, fragments, and embedded credentials are rejected rather
   * than silently changing where a bearer key is sent.
   *
   * Returns the canonical origin or null for invalid input.
   */
  function normalizeServer(value) {
    if (typeof value !== "string" || !value.trim()) return null;
    var parsed;
    try {
      parsed = new URL(value.trim());
    } catch (_error) {
      return null;
    }

    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
    if (parsed.protocol === "http:" && !isLoopbackHostname(parsed.hostname)) return null;
    if (!parsed.hostname || parsed.username || parsed.password) return null;
    if (parsed.search || parsed.hash) return null;
    // API routes are mounted at the server root.  Rejecting a path avoids
    // accidentally validating one origin and later sending credentials to a
    // surprising sub-path.  A trailing slash is harmless and canonicalised.
    if (parsed.pathname && parsed.pathname !== "/") return null;
    return parsed.origin;
  }

  /** Validate bookmark targets without touching the network. */
  function validBookmarkURL(value) {
    if (typeof value !== "string" || !value.trim()) return false;
    var parsed;
    try {
      parsed = new URL(value.trim());
    } catch (_error) {
      return false;
    }
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return false;
    if (!parsed.hostname || parsed.username || parsed.password) return false;
    // URL() rejects most control characters, but explicitly check the input
    // too so credentials are never sent for a malformed value.
    if (/[\u0000-\u001f\u007f]/.test(value)) return false;
    return true;
  }

  function canonicalBookmarkURL(value) {
    if (!validBookmarkURL(value)) return null;
    try {
      return new URL(String(value).trim()).href;
    } catch (_error) {
      return null;
    }
  }

  // Omitted ports in Chrome match patterns mean every port, not the default.
  // Build the string directly: URL serialization would remove :443/:80 again.
  function permissionPattern(server) {
    var normalized = normalizeServer(server);
    if (!normalized) throw new Error("Invalid Boopmark server origin.");
    var url = new URL(normalized);
    return url.protocol + "//" + url.hostname + ":" + (url.port || (url.protocol === "https:" ? "443" : "80")) + "/*";
  }

  /** Trim comma-separated tags and discard blank entries. */
  function parseTags(value) {
    var values = Array.isArray(value) ? value : String(value == null ? "" : value).split(",");
    return values
      .map(function (tag) { return String(tag == null ? "" : tag).trim(); })
      .filter(function (tag) { return tag.length > 0; });
  }

  function formatTags(value) {
    return parseTags(value).join(", ");
  }

  function cloneDirty(dirty) {
    var result = { title: false, description: false, tags: false, url: false };
    if (!dirty || typeof dirty !== "object") return result;
    FIELD_NAMES.forEach(function (field) {
      result[field] = dirty[field] === true;
    });
    result.url = dirty.url === true;
    return result;
  }

  function cloneGenerated(generated) {
    var result = { title: false, description: false, tags: false };
    if (!generated || typeof generated !== "object") return result;
    FIELD_NAMES.forEach(function (field) {
      result[field] = generated[field] === true;
    });
    return result;
  }

  function touchDraft(draft) {
    var next = Object.assign({}, draft);
    next.revision = Math.max(0, Number(draft && draft.revision) || 0) + 1;
    next.updatedAt = Date.now();
    return next;
  }

  /** Create the session draft for one toolbar capture. */
  function createDraft(context) {
    context = context || {};
    var capturedUrl = typeof context.url === "string" ? context.url.trim() : "";
    var browserTitle = typeof context.title === "string" ? context.title : "";
    var server = typeof context.server === "string" ? context.server : "";
    var draft = {
      version: 1,
      id: typeof context.id === "string" && context.id ? context.id : makeDraftId(context),
      server: server,
      connectionEpoch: context.connectionEpoch || "",
      tabId: Number.isInteger(context.tabId) ? context.tabId : null,
      originalUrl: capturedUrl,
      url: capturedUrl,
      title: browserTitle,
      description: "",
      tags: "",
      dirty: { title: false, description: false, tags: false, url: false },
      // `generated` lets URL/connection changes clear old server values while
      // retaining values that the user authored.  Browser title is a
      // provisional value, not generated metadata.
      generated: { title: false, description: false, tags: false },
      browserTitle: browserTitle,
      generation: 0,
      suggestionAttempted: false,
      metadataStatus: "empty",
      error: "",
      status: "ready",
      revision: 0,
      updatedAt: Date.now()
    };
    return draft;
  }

  function makeDraftId(context) {
    context = context || {};
    var server = String(context.server || "");
    var tabId = String(Number.isInteger(context.tabId) ? context.tabId : "tab");
    var originalUrl = String(context.originalUrl == null ? context.url || "" : context.originalUrl);
    // encodeURIComponent is available in both extension contexts and Node;
    // unlike a raw URL it is safe as a storage key.
    return "boopmark-draft:" + encodeURIComponent(server) + ":" + encodeURIComponent(tabId) + ":" + encodeURIComponent(originalUrl);
  }

  function normalizeDraft(raw) {
    if (!raw || typeof raw !== "object") return null;
    var draft = Object.assign({}, raw);
    draft.id = String(draft.id || makeDraftId(draft));
    draft.server = String(draft.server || "");
    draft.url = typeof draft.url === "string" ? draft.url : "";
    draft.originalUrl = typeof draft.originalUrl === "string" ? draft.originalUrl : draft.url;
    draft.title = typeof draft.title === "string" ? draft.title : "";
    draft.description = typeof draft.description === "string" ? draft.description : "";
    draft.tags = typeof draft.tags === "string" ? draft.tags : formatTags(draft.tags);
    draft.dirty = cloneDirty(raw.dirty);
    draft.generated = cloneGenerated(draft.generated);
    draft.browserTitle = typeof draft.browserTitle === "string" ? draft.browserTitle : "";
    draft.connectionEpoch = String(draft.connectionEpoch || "");
    draft.generation = Math.max(0, Number(draft.generation) || 0);
    draft.revision = Math.max(0, Number(draft.revision) || 0);
    draft.suggestionAttempted = draft.suggestionAttempted === true;
    draft.metadataStatus = ["loading", "filled", "empty", "error"].indexOf(draft.metadataStatus) >= 0
      ? draft.metadataStatus : "empty";
    if (typeof draft.error === "string") {
      draft.error = draft.error;
    } else if (draft.error && typeof draft.error === "object") {
      draft.error = {
        kind: typeof draft.error.kind === "string" ? draft.error.kind : "error",
        message: typeof draft.error.message === "string" ? draft.error.message.slice(0, 500) : "Could not fetch metadata."
      };
    } else {
      draft.error = "";
    }
    draft.status = typeof draft.status === "string" ? draft.status : "ready";
    return draft;
  }

  /**
   * Mark a visible form field as user-authored.  Empty strings are edits too;
   * this is what prevents a late suggestion from undoing an intentional clear.
   */
  function updateField(draft, field, value) {
    draft = normalizeDraft(draft) || createDraft({});
    var next = Object.assign({}, draft);
    if (field === "url") {
      return changeURL(next, value);
    }
    if (FIELD_NAMES.indexOf(field) < 0) return draft;
    next[field] = field === "tags" ? String(value == null ? "" : value) : String(value == null ? "" : value);
    next.dirty = cloneDirty(draft.dirty);
    next.dirty[field] = true;
    next.generated = cloneGenerated(draft.generated);
    next.generated[field] = false;
    next.error = "";
    return touchDraft(next);
  }

  /**
   * Change the URL and invalidate all prior suggestion generations.  A URL
   * can later change back to its original value; generation is incremented
   * rather than compared by value, so A -> B -> A cannot admit stale data.
   */
  function changeURL(draft, value) {
    draft = normalizeDraft(draft) || createDraft({});
    var next = Object.assign({}, draft);
    next.url = String(value == null ? "" : value);
    next.dirty = cloneDirty(draft.dirty);
    next.dirty.url = true;
    next.generated = cloneGenerated(draft.generated);
    FIELD_NAMES.forEach(function (field) {
      if (!draft.dirty[field]) {
        // The browser title is a fallback for the captured page only.  Do not
        // carry it into a manually entered URL unless it was still the
        // original capture URL.  This also clears generated values from the
        // previous URL; authored values are protected by dirty[field].
        next[field] = field === "title" && next.url === draft.originalUrl ? draft.browserTitle : "";
      }
      next.generated[field] = false;
    });
    next.generation = (Number(draft.generation) || 0) + 1;
    next.suggestionAttempted = false;
    next.metadataStatus = "empty";
    next.status = "ready";
    next.error = "";
    return touchDraft(next);
  }

  /** Rebind a draft after reconnecting; preserve edits but clear generated data. */
  function rebindDraft(draft, connectionEpoch, server) {
    draft = normalizeDraft(draft) || createDraft({});
    var next = Object.assign({}, draft);
    next.server = String(server || draft.server || "");
    next.connectionEpoch = String(connectionEpoch || "");
    next.generated = cloneGenerated(draft.generated);
    FIELD_NAMES.forEach(function (field) {
      if (!draft.dirty[field]) {
        next[field] = field === "title" && next.url === draft.originalUrl ? draft.browserTitle : "";
      }
      next.generated[field] = false;
    });
    next.generation = (Number(draft.generation) || 0) + 1;
    next.suggestionAttempted = false;
    next.metadataStatus = "empty";
    next.status = "ready";
    next.error = "";
    return touchDraft(next);
  }

  /** Begin one automatic/manual suggestion attempt and return its token. */
  function beginSuggestion(draft) {
    draft = normalizeDraft(draft) || createDraft({});
    var next = touchDraft(draft);
    next.generation = (Number(draft.generation) || 0) + 1;
    next.suggestionAttempted = true;
    next.metadataStatus = "loading";
    next.status = "ready";
    next.error = "";
    return {
      draft: next,
      token: requestToken(next)
    };
  }

  function requestToken(draft) {
    draft = normalizeDraft(draft) || {};
    return {
      generation: Number(draft.generation) || 0,
      url: String(draft.url || ""),
      server: String(draft.server || ""),
      connectionEpoch: String(draft.connectionEpoch || "")
    };
  }

  function isCurrentRequest(draft, token) {
    draft = normalizeDraft(draft);
    if (!draft || !token) return false;
    return (Number(draft.generation) || 0) === (Number(token.generation) || 0)
      && String(draft.url || "") === String(token.url || "")
      && String(draft.server || "") === String(token.server || "")
      && String(draft.connectionEpoch || "") === String(token.connectionEpoch || "");
  }

  /**
   * Merge a server suggestion only into fields the user has not edited.
   * Returns `{draft, applied, stale}` so callers can safely ignore late
   * results without having to compare URLs themselves.
   */
  function applySuggestion(draft, suggestion, token) {
    draft = normalizeDraft(draft);
    if (!draft || !isCurrentRequest(draft, token)) return { draft: draft, applied: 0, stale: true };
    if (draft.status === "saving" || draft.status === "success") {
      return { draft: draft, applied: 0, stale: true };
    }
    suggestion = suggestion && typeof suggestion === "object" ? suggestion : {};
    var next = Object.assign({}, draft);
    next.dirty = cloneDirty(draft.dirty);
    next.generated = cloneGenerated(draft.generated);
    var applied = 0;
    var title = typeof suggestion.title === "string" ? suggestion.title.trim() : "";
    var description = typeof suggestion.description === "string" ? suggestion.description.trim() : "";
    var tags = formatTags(suggestion.tags);
    if (!next.dirty.title && title) {
      next.title = title;
      next.generated.title = true;
      applied += 1;
    }
    if (!next.dirty.description && description) {
      next.description = description;
      next.generated.description = true;
      applied += 1;
    }
    if (!next.dirty.tags && tags) {
      next.tags = tags;
      next.generated.tags = true;
      applied += 1;
    }
    next.metadataStatus = applied > 0 ? "filled" : "empty";
    next.error = "";
    next.status = "ready";
    return { draft: touchDraft(next), applied: applied, stale: false };
  }

  function completeSuggestionError(draft, token, message) {
    draft = normalizeDraft(draft);
    if (!draft || !isCurrentRequest(draft, token) || draft.status === "saving") {
      return { draft: draft, stale: true };
    }
    var next = touchDraft(draft);
    next.metadataStatus = "error";
    next.error = String(message || "Could not fetch metadata.");
    next.status = "ready";
    return { draft: next, stale: false };
  }

  function serializeFields(fields) {
    fields = fields || {};
    return {
      url: String(fields.url == null ? "" : fields.url).trim(),
      title: String(fields.title == null ? "" : fields.title),
      description: String(fields.description == null ? "" : fields.description),
      tags: parseTags(fields.tags)
    };
  }

  function serializeCreatePayload(draftOrFields) {
    var payload = serializeFields(draftOrFields || {});
    return payload;
  }

  function operationMarker(fields, context) {
    fields = serializeFields(fields);
    context = context || {};
    var requestedId = typeof context.id === "string" ? context.id.trim() : "";
    return {
      version: 1,
      id: UUID_PATTERN.test(requestedId) ? requestedId : randomId(),
      state: "pending",
      server: String(context.server || ""),
      connectionEpoch: String(context.connectionEpoch || ""),
      draftId: String(context.draftId || ""),
      url: fields.url,
      submittedAt: Number(context.submittedAt) || Date.now(),
      error: ""
    };
  }

  function transitionOperation(marker, outcome) {
    marker = marker && typeof marker === "object" ? Object.assign({}, marker) : null;
    if (!marker) return null;
    outcome = outcome || {};
    if (outcome.state === "success" || outcome.success === true) {
      marker.state = "success";
      if (outcome.bookmarkId != null) marker.bookmarkId = String(outcome.bookmarkId);
      marker.error = "";
    } else if (outcome.state === "unknown" || outcome.unknown === true) {
      marker.state = "unknown";
      marker.error = String(outcome.error || "Save could not be confirmed.");
    } else {
      marker.state = "error";
      marker.error = String(outcome.error || "Could not save bookmark.");
    }
    marker.finishedAt = Date.now();
    return marker;
  }

  function recoverOperation(marker, active) {
    if (!marker || marker.state !== "pending" || active) return marker;
    return transitionOperation(marker, {
      state: "unknown",
      error: "Save could not be confirmed. Check Boopmark before retrying."
    });
  }

  function randomId() {
    try {
      if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
        var generated = crypto.randomUUID();
        if (UUID_PATTERN.test(generated)) return generated;
      }
    } catch (_error) { /* fall through */ }
    // Chrome supports crypto.randomUUID(), but retain a valid UUID fallback
    // for restricted test/webview contexts so every save can be deduplicated.
    return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (character) {
      var random = Math.random() * 16 | 0;
      var value = character === "x" ? random : (random & 0x3 | 0x8);
      return value.toString(16);
    });
  }

  return {
    FIELD_NAMES: FIELD_NAMES.slice(),
    isLoopbackHostname: isLoopbackHostname,
    normalizeServer: normalizeServer,
    permissionPattern: permissionPattern,
    normalizeServerUrl: normalizeServer,
    serverOrigin: normalizeServer,
    validBookmarkURL: validBookmarkURL,
    validateBookmarkUrl: validBookmarkURL,
    validURL: validBookmarkURL,
    canonicalBookmarkURL: canonicalBookmarkURL,
    parseTags: parseTags,
    formatTags: formatTags,
    createDraft: createDraft,
    normalizeDraft: normalizeDraft,
    makeDraftId: makeDraftId,
    updateField: updateField,
    changeURL: changeURL,
    rebindDraft: rebindDraft,
    beginSuggestion: beginSuggestion,
    beginAutofill: beginSuggestion,
    requestToken: requestToken,
    makeRequestToken: requestToken,
    isCurrentRequest: isCurrentRequest,
    applySuggestion: applySuggestion,
    mergeSuggestion: applySuggestion,
    completeSuggestionError: completeSuggestionError,
    serializeFields: serializeFields,
    serializeCreatePayload: serializeCreatePayload,
    operationMarker: operationMarker,
    transitionOperation: transitionOperation,
    recoverOperation: recoverOperation,
    randomId: randomId
  };
});
