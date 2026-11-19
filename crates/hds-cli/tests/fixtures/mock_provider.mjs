// Mock-провайдер для тестов CLI (A3.3): OpenAI-совместимый, слушает
// 127.0.0.1:0, печатает `PORT <n>` и складывает тела запросов в файл
// (MOCK_REQUESTS) — тест проверяет, что к «модели» ушли только метки.
//
// Сценарий — MOCK_SCRIPT (JSON-массив): {toolCalls: [{name, arguments}]} | {text}.
// Запросы сверх сценария повторяют последний шаг.
import http from 'node:http';
import fs from 'node:fs';

const log = process.env.MOCK_REQUESTS;
const script = JSON.parse(process.env.MOCK_SCRIPT ?? '[]');
const usage = { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 };
let seq = 0;

function callsOf(step, n) {
  return (step.toolCalls ?? []).map((call, i) => ({
    index: i,
    id: `call_${n}_${i}`,
    type: 'function',
    function: { name: call.name, arguments: JSON.stringify(call.arguments ?? {}) },
  }));
}

function replyJson(res, step, n) {
  const calls = callsOf(step, n);
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(
    JSON.stringify({
      id: `chatcmpl-${n}`,
      object: 'chat.completion',
      created: 0,
      model: 'mock-model',
      choices: [
        {
          index: 0,
          finish_reason: calls.length ? 'tool_calls' : 'stop',
          message: {
            role: 'assistant',
            content: step.text ?? '',
            ...(calls.length ? { tool_calls: calls.map(({ index, ...rest }) => rest) } : {}),
          },
        },
      ],
      usage,
    }),
  );
}

function replyStream(res, step, n) {
  const calls = callsOf(step, n);
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  const base = { id: `chatcmpl-${n}`, object: 'chat.completion.chunk', created: 0, model: 'mock-model' };
  const send = (chunk) =>
    res.write(`data: ${typeof chunk === 'string' ? chunk : JSON.stringify(chunk)}\n\n`);
  send({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] });
  for (const piece of String(step.text ?? '').match(/[\s\S]{1,12}/g) ?? []) {
    send({ ...base, choices: [{ index: 0, delta: { content: piece }, finish_reason: null }] });
  }
  for (const call of calls) {
    send({ ...base, choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] });
  }
  send({
    ...base,
    choices: [{ index: 0, delta: {}, finish_reason: calls.length ? 'tool_calls' : 'stop' }],
    usage,
  });
  send('[DONE]');
  res.end();
}

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', (chunk) => chunks.push(chunk));
  req.on('end', () => {
    const raw = Buffer.concat(chunks).toString('utf8');
    if (log) fs.appendFileSync(log, `${raw}\n`);
    let body = {};
    try {
      body = JSON.parse(raw);
    } catch {
      body = {};
    }
    seq += 1;
    const step = script[Math.min(seq - 1, script.length - 1)] ?? { text: 'ok' };
    if (body.stream === true) replyStream(res, step, seq);
    else replyJson(res, step, seq);
  });
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(`PORT ${server.address().port}\n`);
});
