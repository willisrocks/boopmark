#!/usr/bin/env node
// A fake llama.cpp `llama-server` for the E2E suite. The E2E server pins
// every LLM request here (`LLM_BACKEND=llamacpp`), so no test can reach a
// paid vendor. It answers by the shape of the request's JSON schema, the way
// the server's use cases ask:
//
//   merges    whole-list consolidation: `wfh` ~ `remote work` at medium
//   pairs     pair check: echoes both tags, judged from VERDICTS below
//   existing  save-time suggestion: the first listed tag
//   prompt    image prompt assistance
import http from "node:http";

const port = Number(process.env.FAKE_LLM_PORT || 4011);

// Pairs the fake model calls the same tag, by confidence.
const VERDICTS = new Map([
  ["javascript|js", "high"],
  ["k8s|kubernetes", "high"],
  ["remote work|wfh", "medium"],
]);

function verdict(a, b) {
  return VERDICTS.get([a.toLowerCase(), b.toLowerCase()].sort().join("|"));
}

// The first JSON string after `marker` in a prompt line.
function quotedAfter(line, marker) {
  const rest = line.slice(line.indexOf(marker) + marker.length);
  const match = rest.match(/^"(?:[^"\\]|\\.)*"/);
  return match ? JSON.parse(match[0]) : "";
}

function answer(schema, messages) {
  const properties = (schema && schema.properties) || {};
  const user = (messages.find((m) => m.role === "user") || {}).content || "";
  if (properties.merges) {
    const merges = user.includes('"wfh"') && user.includes('"remote work"')
      ? [{ canonical: "remote work", members: ["remote work", "wfh"], confidence: "medium" }]
      : [];
    return { merges };
  }
  if (properties.pairs) {
    const pairs = user
      .split("\n")
      .filter((line) => /^\d+\. A: /.test(line))
      .map((line) => {
        const a = quotedAfter(line, "A: ");
        const b = quotedAfter(line, "| B: ");
        const confidence = verdict(a, b);
        return { id: Number(line.split(".")[0]), a, b, same: Boolean(confidence), confidence: confidence || "high" };
      });
    return { pairs };
  }
  if (properties.prompt) {
    return { prompt: "A calm, abstract illustration in soft colors" };
  }
  const listed = (properties.existing && properties.existing.items.enum) || [];
  return {
    title: "Fake suggested title",
    description: "Fake suggested description.",
    ...(properties.existing ? { existing: listed.slice(0, 1) } : {}),
    new: [],
  };
}

const server = http.createServer((req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok");
    return;
  }
  if (req.method !== "POST" || req.url !== "/v1/chat/completions") {
    res.writeHead(404);
    res.end();
    return;
  }
  let body = "";
  req.on("data", (chunk) => { body += chunk; });
  req.on("end", () => {
    let request;
    try {
      request = JSON.parse(body);
    } catch {
      res.writeHead(400);
      res.end();
      return;
    }
    const schema = request.response_format && request.response_format.json_schema
      ? request.response_format.json_schema.schema
      : {};
    const content = JSON.stringify(answer(schema, request.messages || []));
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      choices: [{ message: { role: "assistant", content }, finish_reason: "stop" }],
      usage: { prompt_tokens: 10, completion_tokens: 5 },
    }));
  });
});

server.listen(port, "127.0.0.1", () => {
  console.log(`fake LLM listening on http://127.0.0.1:${port}`);
});
