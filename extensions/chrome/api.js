(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory(require("./core.js"));
  } else {
    root.BoopmarkApi = factory(root.BoopmarkCore);
  }
})(typeof globalThis !== "undefined" ? globalThis : this, function (Core) {
  "use strict";

  if (!Core) throw new Error("BoopmarkCore is required by BoopmarkApi");
  var UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

  class ApiError extends Error {
    constructor(kind, message, status) {
      super(message);
      this.name = "ApiError";
      this.kind = kind || "transport";
      this.status = Number(status) || 0;
    }

    toJSON() {
      return { kind: this.kind, message: this.message, status: this.status };
    }
  }

  function errorMessage(status, body) {
    if (body && typeof body === "object" && typeof body.error === "string" && body.error.trim()) {
      return body.error.trim().slice(0, 500);
    }
    if (status) return "Boopmark returned HTTP " + status + ".";
    return "Could not reach Boopmark.";
  }

  async function readBody(response) {
    if (response && typeof response.text !== "function" && typeof response.json === "function") {
      try { return await response.json(); } catch (_error) { return null; }
    }
    var text = "";
    try {
      text = await response.text();
    } catch (_error) {
      return null;
    }
    if (!text) return null;
    try {
      return JSON.parse(text);
    } catch (_error) {
      return { _text: text.slice(0, 500) };
    }
  }

  function buildApiURL(server, path) {
    var normalized = Core.normalizeServer(server);
    if (!normalized) throw new ApiError("configuration", "Enter a valid HTTPS Boopmark server URL.");
    if (typeof path !== "string" || path.charAt(0) !== "/" || path.indexOf("//") === 0) {
      throw new ApiError("configuration", "Invalid Boopmark API path.");
    }
    // `normalized` is an origin, so URL() cannot inherit an untrusted path,
    // query, or fragment from user input.
    return new URL(path, normalized).toString();
  }

  function canonicalURL(value) {
    if (!Core.validBookmarkURL(value)) throw new ApiError("validation", "Open a web page to add a bookmark.");
    // URL() is used only for validation.  Keep the reviewed string exactly as
    // entered so query/fragment text and harmless casing/default-port choices
    // are not silently rewritten before the server receives the snapshot.
    return String(value).trim();
  }

  class BoopmarkApiClient {
    constructor(options) {
      options = options || {};
      this.server = Core.normalizeServer(options.server || options.serverUrl);
      if (!this.server) throw new ApiError("configuration", "Enter a valid HTTPS Boopmark server URL.");
      this.apiKey = typeof options.apiKey === "string" ? options.apiKey.trim() : "";
      if (!this.apiKey) throw new ApiError("configuration", "Enter your Boopmark API key.");
      this.fetchImpl = options.fetchImpl || (typeof fetch === "function" ? fetch.bind(globalThis) : null);
      if (!this.fetchImpl) throw new ApiError("configuration", "This browser cannot make network requests.");
    }

    async request(method, path, body, requestOptions) {
      requestOptions = requestOptions || {};
      if (requestOptions.idempotencyKey !== undefined && requestOptions.idempotencyKey !== null
          && (typeof requestOptions.idempotencyKey !== "string"
            || !UUID_PATTERN.test(requestOptions.idempotencyKey.trim()))) {
        throw new ApiError("validation", "Invalid save operation ID.");
      }
      var url = buildApiURL(this.server, path);
      var headers = {
        Accept: "application/json",
        Authorization: "Bearer " + this.apiKey
      };
      if (requestOptions.idempotencyKey !== undefined && requestOptions.idempotencyKey !== null) {
        headers["Idempotency-Key"] = requestOptions.idempotencyKey.trim();
      }
      var init = {
        method: method,
        headers: headers,
        credentials: "omit",
        redirect: "error",
        referrerPolicy: "no-referrer",
        cache: "no-store"
      };
      if (body !== undefined && body !== null) {
        headers["Content-Type"] = "application/json";
        init.body = JSON.stringify(body);
      }
      if (requestOptions.signal) init.signal = requestOptions.signal;

      var response;
      try {
        response = await this.fetchImpl(url, init);
      } catch (error) {
        // A network interruption after a create is dispatched cannot tell us
        // whether the server committed it.  Callers must treat it as unknown.
        var kind = requestOptions.operation === "save" ? "unknown" : "transport";
        var message = requestOptions.operation === "save"
          ? "Save could not be confirmed. Check Boopmark before retrying."
          : "Could not reach Boopmark.";
        if (error && error.name === "AbortError" && requestOptions.operation !== "save") {
          message = "Metadata request was cancelled.";
        }
        throw new ApiError(kind, message, 0);
      }

      var bodyData = await readBody(response);
      var status = response && Number(response.status) || 0;
      var responseOk = response && (typeof response.ok === "boolean" ? response.ok : status >= 200 && status < 300);
      if (!response || !responseOk) {
        var message = errorMessage(status, bodyData);
        if (status === 401 || status === 403) {
          throw new ApiError("auth", "Reconnect Boopmark.", status);
        }
        // A server-side 5xx response can be emitted after persistence, so it
        // is conservative to classify it as unknown for a create operation.
        if (requestOptions.operation === "save" && status >= 500) {
          throw new ApiError("unknown", "Save could not be confirmed. Check Boopmark before retrying.", status);
        }
        throw new ApiError(requestOptions.operation === "save" ? "definite" : "server", message, status);
      }

      if (bodyData && bodyData._text !== undefined) {
        var malformedKind = requestOptions.operation === "save" ? "unknown" : "server";
        throw new ApiError(malformedKind, requestOptions.operation === "save"
          ? "Save could not be confirmed. Check Boopmark before retrying."
          : "Boopmark returned an invalid response.", Number(response.status) || 0);
      }
      return requestOptions.withMeta
        ? { data: bodyData, status: Number(response.status) || 0 }
        : bodyData;
    }

    async validateConnection() {
      var body = await this.request("GET", "/api/v1/bookmarks?limit=1", null, { operation: "validate" });
      if (!Array.isArray(body)) {
        throw new ApiError("server", "Boopmark returned an invalid response.");
      }
      return true;
    }

    // Descriptive aliases make the client convenient for small integration
    // harnesses while keeping the worker's wire operations terse.
    async validate() { return this.validateConnection(); }

    async suggest(value, options) {
      var url = canonicalURL(value);
      var body = await this.request("POST", "/api/v1/bookmarks/suggest", { url: url }, {
        operation: "suggest",
        signal: options && options.signal
      });
      if (!body || typeof body !== "object") {
        throw new ApiError("server", "Boopmark returned an invalid suggestion response.");
      }
      return {
        title: typeof body.title === "string" ? body.title : null,
        description: typeof body.description === "string" ? body.description : null,
        tags: Array.isArray(body.tags) ? body.tags : [],
        imageUrl: typeof body.imageUrl === "string" ? body.imageUrl
          : typeof body.image_url === "string" ? body.image_url : null,
        domain: typeof body.domain === "string" ? body.domain : null
      };
    }

    async suggestBookmark(value, options) { return this.suggest(value, options); }

    async create(fields, options) {
      var payload = Core.serializeCreatePayload(fields);
      payload.url = canonicalURL(payload.url);
      // Do not add `suggest=true`: all visible values are reviewed form
      // values, including intentional empty strings and an empty tag array.
      var result = await this.request("POST", "/api/v1/bookmarks", payload, {
        operation: "save",
        withMeta: true,
        idempotencyKey: options && options.idempotencyKey
      });
      // The create contract is specifically 201 with a created record.  A
      // 2xx without an id (or a 200 proxy response) cannot prove that a
      // bookmark was created and must remain an unknown outcome.
      if (!result || result.status !== 201 || !result.data || typeof result.data !== "object"
          || result.data.id == null || String(result.data.id).trim() === "") {
        throw new ApiError("unknown", "Save could not be confirmed. Check Boopmark before retrying.", result && result.status);
      }
      return result.data;
    }

    async createBookmark(fields, options) { return this.create(fields, options); }
  }

  return {
    ApiError: ApiError,
    BoopmarkApiClient: BoopmarkApiClient,
    ApiClient: BoopmarkApiClient,
    buildApiURL: buildApiURL,
    canonicalURL: canonicalURL
  };
});
