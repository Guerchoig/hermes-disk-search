// Mock-провайдер для спайков A0 (S1–S7).
//
// Отвечает в формате OpenAI Chat Completions (/v1/chat/completions) и Anthropic
// Messages (/v1/messages), поддерживает SSE-стрим и обычный JSON. Ответы —
// скриптованные: сценарий задаётся файлом JSON (--script), каждый входящий
// запрос получает следующий шаг сценария.
//
// Пишет журнал запросов (--log, JSONL): путь, признаки авторизации (без самого
// ключа), разобранное тело запроса к модели. В сеть сам не ходит.
//
// Usage:
//   node mock_provider.mjs --port 0 --script script.json --log requests.jsonl
//   node mock_provider.mjs --port 8788 --text ok
//
// Script format (JSON array), один элемент — один ответ модели:
//   { "text": "готово" }
//   { "text": "читаю", "toolCalls": [{ "name": "read_files", "arguments": {"path": "a.txt"} }] }
// Запросы сверх сценария повторяют последний шаг (в журнале stepOutOfRange=true).

import http from 'node:http';
import { appendFileSync, readFileSync, existsSync } from 'node:fs';

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  const value = process.argv[i + 1];
  return value === undefined || value.startsWith('--') ? true : value;
}

const port = Number(arg('port', '0'));
const scriptPath = arg('script', null);
const logPath = arg('log', null);
const defaultText = String(arg('text', 'ok'));

const script = scriptPath && existsSync(scriptPath)
  ? JSON.parse(readFileSync(scriptPath, 'utf8'))
  : [{ text: defaultText }];

const state = { requestCount: 0, byPath: {}, started: new Date().toISOString() };

function log(entry) {
  if (!logPath) return;
  appendFileSync(logPath, `${JSON.stringify(entry)}\n`, 'utf8');
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function nextStep() {
  // requestCount уже увеличен обработчиком, поэтому индекс шага = requestCount - 1
  const index = Math.max(0, state.requestCount - 1);
  const step = script[Math.min(index, script.length - 1)] ?? { text: defaultText };
  return { step, outOfRange: index >= script.length };
}

function textOfContent(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content
      .map((part) => {
        if (typeof part === 'string') return part;
        if (part?.type === 'text') return part.text;
        if (part?.type === 'tool_result') {
          return `[tool_result ${part.tool_use_id ?? part.tool_call_id ?? ''}] ${textOfContent(part.content)}`;
        }
        if (part?.type === 'tool_use') return `[tool_use ${part.name}]`;
        return '';
      })
      .join('\n');
  }
  return '';
}

function messagesArray(parsed) {
  const list = Array.isArray(parsed.messages) ? parsed.messages : [];
  if (Array.isArray(parsed.system)) return [{ role: 'system', content: parsed.system }, ...list];
  return list;
}

function extractLastUserText(parsed) {
  const list = messagesArray(parsed);
  for (let i = list.length - 1; i >= 0; i -= 1) {
    if (list[i]?.role === 'user') return textOfContent(list[i].content).slice(0, 8000);
  }
  return '';
}

function extractToolResults(parsed) {
  const out = [];
  for (const m of messagesArray(parsed)) {
    if (m?.role === 'tool' || m?.role === 'function') {
      out.push(textOfContent(m.content).slice(0, 2000));
      continue;
    }
    if (Array.isArray(m?.content)) {
      for (const part of m.content) {
        if (part?.type === 'tool_result') out.push(textOfContent(part.content).slice(0, 2000));
      }
    }
  }
  return out;
}

function summarizeBody(raw) {
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { parseError: true, bytes: raw.length };
  }
  return {
    model: parsed.model,
    stream: parsed.stream ?? false,
    messageCount: Array.isArray(parsed.messages) ? parsed.messages.length : undefined,
    hasTools: Array.isArray(parsed.tools) ? parsed.tools.length : 0,
    toolNames: Array.isArray(parsed.tools)
      ? parsed.tools.map((t) => t?.function?.name ?? t?.name).filter(Boolean)
      : [],
    maxTokens: parsed.max_tokens ?? parsed.max_completion_tokens,
    // для S3/S4/S6: что модель видит в последней реплике и в результатах инструментов
    lastUserText: extractLastUserText(parsed),
    toolResultSnippets: extractToolResults(parsed),
    bodyFull: raw.length <= 200_000 ? parsed : undefined,
    bytes: raw.length,
  };
}

function openAiResponse(step) {
  const toolCalls = Array.isArray(step.toolCalls) ? step.toolCalls : [];
  return {
    id: `chatcmpl-${state.requestCount}`,
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model: 'mock-model',
    choices: [
      {
        index: 0,
        message: {
          role: 'assistant',
          content: step.text ?? '',
          ...(toolCalls.length
            ? {
                tool_calls: toolCalls.map((call, i) => ({
                  id: `call_${state.requestCount}_${i}`,
                  type: 'function',
                  function: { name: call.name, arguments: JSON.stringify(call.arguments ?? {}) },
                })),
              }
            : {}),
        },
        finish_reason: toolCalls.length ? 'tool_calls' : 'stop',
      },
    ],
    usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 },
  };
}

function sseWrite(res, obj) {
  res.write(`data: ${typeof obj === 'string' ? obj : JSON.stringify(obj)}\n\n`);
}

function serveOpenAiStream(res, step) {
  const toolCalls = Array.isArray(step.toolCalls) ? step.toolCalls : [];
  res.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    connection: 'keep-alive',
  });
  const base = {
    id: `chatcmpl-${state.requestCount}`,
    object: 'chat.completion.chunk',
    created: Math.floor(Date.now() / 1000),
    model: 'mock-model',
  };

  sseWrite(res, { ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] });
  for (const chunk of String(step.text ?? '').match(/[\s\S]{1,12}/g) ?? []) {
    sseWrite(res, { ...base, choices: [{ index: 0, delta: { content: chunk }, finish_reason: null }] });
  }
  toolCalls.forEach((call, i) => {
    const toolCall = {
      index: i,
      id: `call_${state.requestCount}_${i}`,
      type: 'function',
      function: { name: call.name, arguments: JSON.stringify(call.arguments ?? {}) },
    };
    sseWrite(res, { ...base, choices: [{ index: 0, delta: { tool_calls: [toolCall] }, finish_reason: null }] });
  });
  const finish = toolCalls.length ? 'tool_calls' : 'stop';
  sseWrite(res, {
    ...base,
    choices: [{ index: 0, delta: {}, finish_reason: finish }],
    usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 },
  });
  sseWrite(res, '[DONE]');
  res.end();
}


function anthropicJson(step) {
  const toolCalls = Array.isArray(step.toolCalls) ? step.toolCalls : [];
  const content = [];
  if (step.text) content.push({ type: 'text', text: step.text });
  for (const call of toolCalls) {
    content.push({
      type: 'tool_use',
      id: `toolu_${state.requestCount}_${call.name}`,
      name: call.name,
      input: call.arguments ?? {},
    });
  }
  return {
    id: `msg_${state.requestCount}`,
    type: 'message',
    role: 'assistant',
    model: 'mock-model',
    content,
    stop_reason: toolCalls.length ? 'tool_use' : 'end_turn',
    stop_sequence: null,
    usage: { input_tokens: 100, output_tokens: 20 },
  };
}

function serveAnthropicStream(res, step) {
  const toolCalls = Array.isArray(step.toolCalls) ? step.toolCalls : [];
  res.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    connection: 'keep-alive',
  });
  const send = (event, data) => res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);

  send('message_start', {
    type: 'message_start',
    message: {
      id: `msg_${state.requestCount}`,
      type: 'message',
      role: 'assistant',
      model: 'mock-model',
      content: [],
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 100, output_tokens: 1 },
    },
  });

  let index = 0;
  if (step.text) {
    send('content_block_start', {
      type: 'content_block_start',
      index,
      content_block: { type: 'text', text: '' },
    });
    for (const chunk of String(step.text).match(/[\s\S]{1,12}/g) ?? []) {
      send('content_block_delta', {
        type: 'content_block_delta',
        index,
        delta: { type: 'text_delta', text: chunk },
      });
    }
    send('content_block_stop', { type: 'content_block_stop', index });
    index += 1;
  }
  for (const call of toolCalls) {
    send('content_block_start', {
      type: 'content_block_start',
      index,
      content_block: {
        type: 'tool_use',
        id: `toolu_${state.requestCount}_${call.name}`,
        name: call.name,
        input: {},
      },
    });
    send('content_block_delta', {
      type: 'content_block_delta',
      index,
      delta: { type: 'input_json_delta', partial_json: JSON.stringify(call.arguments ?? {}) },
    });
    send('content_block_stop', { type: 'content_block_stop', index });
    index += 1;
  }
  send('message_delta', {
    type: 'message_delta',
    delta: { stop_reason: toolCalls.length ? 'tool_use' : 'end_turn', stop_sequence: null },
    usage: { output_tokens: 20 },
  });
  send('message_stop', { type: 'message_stop' });
  res.end();
}

const server = http.createServer(async (req, res) => {
  const url = req.url ?? '/';

  if (req.method === 'GET' && url.startsWith('/__stats')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ ...state, scriptLength: script.length }, null, 2));
    return;
  }
  if (req.method === 'GET' && url.startsWith('/__reset')) {
    state.requestCount = 0;
    state.byPath = {};
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end('{"reset":true}');
    return;
  }

  const raw = await readBody(req);
  state.requestCount += 1;
  state.byPath[url] = (state.byPath[url] ?? 0) + 1;
  const { step, outOfRange } = nextStep();

  let body = {};
  try {
    body = JSON.parse(raw);
  } catch {
    body = {};
  }

  const isAnthropic = url.includes('/messages');
  log({
    at: new Date().toISOString(),
    method: req.method,
    url,
    authHeaderPresent: Boolean(req.headers.authorization || req.headers['x-api-key']),
    authScheme: (req.headers.authorization ?? req.headers['x-api-key'] ?? '').split(' ')[0] || null,
    contentType: req.headers['content-type'] ?? null,
    stepIndex: state.requestCount - 1,
    outOfRange,
    ...summarizeBody(raw),
  });

  const wantsStream = body.stream === true || url.includes('stream=true');

  // Сценарий ошибки: {"status": 429, "retryAfter": 1, "error": "rate limit"}.
  // Нужен, чтобы проверить, что повторы провайдера тоже идут через наш fetch.
  if (typeof step.status === 'number' && step.status >= 400) {
    const message = step.error ?? 'mock provider error';
    res.writeHead(step.status, {
      'content-type': 'application/json',
      ...(step.retryAfter ? { 'retry-after': String(step.retryAfter) } : {}),
    });
    res.end(
      JSON.stringify(
        isAnthropic
          ? { type: 'error', error: { type: 'api_error', message } }
          : { error: { message, type: 'mock_error', code: step.status } },
      ),
    );
    return;
  }

  if (isAnthropic) {
    if (wantsStream) serveAnthropicStream(res, step);
    else {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(anthropicJson(step)));
    }
    return;
  }
  if (wantsStream) {
    serveOpenAiStream(res, step);
    return;
  }
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify(openAiResponse(step)));
});

server.listen(port, '127.0.0.1', () => {
  const actual = server.address().port;
  console.log(`mock-provider listening on http://127.0.0.1:${actual}`);
  console.log(`script entries: ${script.length}${scriptPath ? ` (${scriptPath})` : ' (built-in)'}`);
});

