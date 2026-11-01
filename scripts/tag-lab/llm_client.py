"""Python mirror of the server's shared LLM port (server/src/domain/ports/llm_client.rs).

    complete_structured(StructuredRequest) -> StructuredResponse(json, usage)

with one adapter per backend, chosen by `provider`:

  anthropic  Messages API, output_config.format json_schema   (ANTHROPIC_API_KEY)
  openai     Responses API, strict text.format json_schema      (OPENAI_API_KEY)
  llamacpp   local llama-server, OpenAI-compatible chat completions with a
             grammar-constrained response_format (free; --base-url)

Keys are read from the environment inside the adapter and never returned,
logged or written. Errors carry only the HTTP status, never a response body.
Standard library only, Python 3.9+.
"""
import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass, field

# Keywords structured-output engines do not enforce; removed before sending to
# hosted vendors and enforced by the caller (same list as the Rust adapter).
UNENFORCED = {"maxItems", "minItems", "minLength", "maxLength", "minimum", "maximum", "multipleOf"}


@dataclass
class StructuredRequest:
    provider: str
    model: str
    system: str
    user: str
    json_schema: dict
    max_output_tokens: int
    # Not part of the Rust request: the Rust adapters get these from config.
    effort: str = ""
    base_url: str = "http://127.0.0.1:4211"


@dataclass
class TokenUsage:
    input_tokens: int = 0
    output_tokens: int = 0


@dataclass
class StructuredResponse:
    json: dict
    usage: TokenUsage = field(default_factory=TokenUsage)
    # Vendor stop reason ("end_turn", "completed", ...), kept for diagnostics.
    stop_reason: str = ""


class LlmError(Exception):
    """args: (message[, TokenUsage billed before the failure]). `fatal` marks
    errors that will repeat on every call (no key, rejected key, unknown
    model, invalid request), so a caller should stop instead of retrying."""
    fatal = False


class FatalLlmError(LlmError):
    fatal = True


def strip_unenforced(schema):
    if isinstance(schema, dict):
        return {k: strip_unenforced(v) for k, v in schema.items() if k not in UNENFORCED}
    if isinstance(schema, list):
        return [strip_unenforced(v) for v in schema]
    return schema


def _key(name):
    value = os.environ.get(name, "").strip()
    if not value:
        raise FatalLlmError(f"{name} is not set")
    return value


def _post(url, headers, body, timeout):
    req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST",
                                 headers={"Content-Type": "application/json", **headers})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        error = FatalLlmError if e.code in (400, 401, 403, 404) else LlmError
        raise error(f"HTTP {e.code} from {url.split('/')[2]}") from None
    except urllib.error.URLError as e:
        raise LlmError(f"cannot reach {url.split('/')[2]}: {e.reason}") from None


def _parse(text, vendor, usage):
    try:
        return json.loads(text.strip())
    except (ValueError, AttributeError):
        raise LlmError(f"{vendor} returned invalid structured JSON", usage) from None


def _anthropic(req):
    output_config = {"format": {"type": "json_schema", "schema": strip_unenforced(req.json_schema)}}
    if req.effort and not req.model.startswith(("claude-haiku-4", "claude-3")):
        output_config["effort"] = req.effort
    body = {"model": req.model, "max_tokens": req.max_output_tokens,
            "messages": [{"role": "user", "content": req.user}], "output_config": output_config}
    if req.system:
        body["system"] = req.system
    resp = _post("https://api.anthropic.com/v1/messages",
                 {"x-api-key": _key("ANTHROPIC_API_KEY"), "anthropic-version": "2023-06-01"}, body, timeout=600)
    u = resp.get("usage", {})
    usage = TokenUsage(u.get("input_tokens", 0), u.get("output_tokens", 0))
    stop = resp.get("stop_reason")
    if stop in ("refusal", "max_tokens"):
        raise LlmError(f"Anthropic stopped with {stop}", usage)
    # Thinking blocks may come first; the answer is the text block.
    text = next((b.get("text", "") for b in resp.get("content", []) if b.get("type") == "text"), "")
    return StructuredResponse(_parse(text, "Anthropic", usage), usage, stop or "")


def _openai(req):
    resp = _post("https://api.openai.com/v1/responses", {"Authorization": f"Bearer {_key('OPENAI_API_KEY')}"},
                 {"model": req.model, "instructions": req.system or None, "input": req.user,
                  "max_output_tokens": req.max_output_tokens, "store": False,
                  "text": {"format": {"type": "json_schema", "name": "result", "strict": True,
                                      "schema": strip_unenforced(req.json_schema)}}},
                 timeout=600)
    u = resp.get("usage", {})
    usage = TokenUsage(u.get("input_tokens", 0), u.get("output_tokens", 0))
    if resp.get("status") == "incomplete":
        raise LlmError("OpenAI response incomplete", usage)
    text = resp.get("output_text") or "".join(
        c.get("text", "") for item in resp.get("output", []) if item.get("type") == "message"
        for c in item.get("content", []))
    return StructuredResponse(_parse(text, "OpenAI", usage), usage, resp.get("status") or "")


def _llamacpp(req):
    resp = _post(f"{req.base_url.rstrip('/')}/v1/chat/completions", {},
                 {"model": req.model, "max_tokens": req.max_output_tokens, "temperature": 0,
                  "messages": ([{"role": "system", "content": req.system}] if req.system else [])
                  + [{"role": "user", "content": req.user}],
                  "response_format": {"type": "json_schema", "json_schema": {
                      "name": "result", "strict": True, "schema": req.json_schema}}},
                 timeout=1800)
    u = resp.get("usage", {})
    usage = TokenUsage(u.get("prompt_tokens", 0), u.get("completion_tokens", 0))
    return StructuredResponse(_parse(resp["choices"][0]["message"]["content"], "llama.cpp", usage),
                              usage, resp["choices"][0].get("finish_reason") or "")


ADAPTERS = {"anthropic": _anthropic, "openai": _openai, "llamacpp": _llamacpp}


def complete_structured(req):
    try:
        adapter = ADAPTERS[req.provider]
    except KeyError:
        raise LlmError(f"unknown provider {req.provider}") from None
    return adapter(req)
