// Скриптованный OpenAI-совместимый провайдер для e2e (по образцу
// tools/parity/agent_a0_sdk/mock_provider.mjs). В сеть не ходит; тела
// запросов хранит в памяти — тест проверяет, что ушло «к модели».
//
// Шаг сценария: { text } или { toolCalls: [{ name, arguments }] }.
// Дополнительно: { status: 429 } — ответ ошибкой (проверка повторов, TR-3.10 д),
// { delayMs: 1500 } — задержка ответа (проверка abort).
// Запросы сверх сценария повторяют последний шаг.
import http from 'node:http';

export function startMockProvider(script) {
  const requests = [];
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8');
      let body = {};
      try {
        body = JSON.parse(raw);
      } catch {
        body = {};
      }
      const authorization = String(req.headers.authorization ?? '');
      requests.push({ url: req.url, raw, body, auth: Boolean(authorization), authorization });
      const step = script[Math.min(requests.length - 1, script.length - 1)] ?? { text: 'ok' };
      const respond = () => {
        if (step.status) serveError(res, step, requests.length);
        else if (body.stream === true) serveStream(res, step, requests.length);
        else serveJson(res, step, requests.length);
      };
      if (step.delayMs) setTimeout(respond, step.delayMs);
      else respond();
    });
  });
  const ready = new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      resolve(`http://127.0.0.1:${server.address().port}`);
    });
  });
  return {
    ready,
    requests,
    stop: () => new Promise((resolve) => server.close(() => resolve())),
  };
}

function toolCallsOf(step, n) {
  return (step.toolCalls ?? []).map((call, i) => ({
    index: i,
    id: `call_${n}_${i}`,
    type: 'function',
    function: { name: call.name, arguments: JSON.stringify(call.arguments ?? {}) },
  }));
}

const USAGE = { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 };

/** Ошибка провайдера ({status: 429}): формат OpenAI, чтобы SDK увидел retry-able ошибку. */
function serveError(res, step, n) {
  res.writeHead(step.status, { 'content-type': 'application/json' });
  res.end(
    JSON.stringify({
      error: {
        message: `mock status ${step.status}`,
        type: 'mock_error',
        code: String(step.status),
      },
      id: `chatcmpl-err-${n}`,
    }),
  );
}

function serveJson(res, step, n) {
  const calls = toolCallsOf(step, n);
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(
    JSON.stringify({
      id: `chatcmpl-${n}`,
      object: 'chat.completion',
      created: Math.floor(Date.now() / 1000),
      model: 'mock-model',
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: step.text ?? '',
            ...(calls.length ? { tool_calls: calls.map(({ index, ...c }) => c) } : {}),
          },
          finish_reason: calls.length ? 'tool_calls' : 'stop',
        },
      ],
      usage: USAGE,
    }),
  );
}

function serveStream(res, step, n) {
  const calls = toolCallsOf(step, n);
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  const base = { id: `chatcmpl-${n}`, object: 'chat.completion.chunk', created: 0, model: 'mock-model' };
  const send = (obj) => res.write(`data: ${typeof obj === 'string' ? obj : JSON.stringify(obj)}\n\n`);
  send({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] });
  for (const chunk of String(step.text ?? '').match(/[\s\S]{1,12}/g) ?? []) {
    send({ ...base, choices: [{ index: 0, delta: { content: chunk }, finish_reason: null }] });
  }
  for (const call of calls) {
    send({ ...base, choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] });
  }
  send({
    ...base,
    choices: [{ index: 0, delta: {}, finish_reason: calls.length ? 'tool_calls' : 'stop' }],
    usage: USAGE,
  });
  send('[DONE]');
  res.end();
}

/** Имена инструментов в теле запроса к модели. */
export function toolNames(request) {
  const tools = Array.isArray(request?.body?.tools) ? request.body.tools : [];
  return tools.map((t) => t?.function?.name ?? t?.name).filter(Boolean);
}
