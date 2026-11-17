// S2 — все HTTP-запросы провайдеров идут через переданный fetch. СТОП-ТОЧКА.
//
// Для трёх провайдеров (openrouter, OpenAI-совместимый с произвольным baseUrl,
// anthropic) каждый прогон: наш fetch считает вызовы, mock-провайдер считает
// пришедшие запросы. «Да» = счётчики равны во всех случаях, включая стрим и
// повторы после 429/500.
import path from 'node:path';
import { applySandboxEnv, startMock, makeEgressFetch, OUT_ROOT, writeResult } from './stand.mjs';

const SANDBOX = path.join(OUT_ROOT, 's2', 'sandbox');
const sandbox = applySandboxEnv(SANDBOX);

const { ClineCore } = await import('@cline/sdk');

const CASES = [
  { name: 'openrouter', providerId: 'openrouter', pathSuffix: '/v1', script: [{ text: 'ok' }] },
  {
    name: 'openai-compatible',
    providerId: 'openai-compatible',
    pathSuffix: '/v1',
    script: [{ text: 'ok' }],
  },
  { name: 'anthropic', providerId: 'anthropic', pathSuffix: '', script: [{ text: 'ok' }] },
  {
    name: 'openrouter-429-retry',
    providerId: 'openrouter',
    pathSuffix: '/v1',
    script: [{ status: 429, retryAfter: 1, error: 'rate limited' }, { text: 'after retry' }],
  },
  {
    name: 'openai-compatible-500-retry',
    providerId: 'openai-compatible',
    pathSuffix: '/v1',
    script: [{ status: 500, error: 'boom' }, { text: 'after retry' }],
  },
  {
    name: 'anthropic-500-retry',
    providerId: 'anthropic',
    pathSuffix: '',
    script: [{ status: 500, error: 'overloaded' }, { text: 'after retry' }],
  },
  {
    name: 'openrouter-3x429',
    providerId: 'openrouter',
    pathSuffix: '/v1',
    script: [
      { status: 429, retryAfter: 1, error: 'rate limited 1' },
      { status: 429, retryAfter: 1, error: 'rate limited 2' },
      { status: 429, retryAfter: 1, error: 'rate limited 3' },
      { text: 'after 3 retries' },
    ],
  },
  {
    // fail-closed: наш fetch вообще отказывается отправлять запрос (аналог
    // privacy_filter_unavailable из TR-6.5) — на mock не должно прийти ни одного
    name: 'egress-blocked',
    providerId: 'openrouter',
    pathSuffix: '/v1',
    script: [{ text: 'never delivered' }],
    blockFetch: true,
  },
];

/** Ждёт событие ended/status=completed|error по сессии. */
async function waitEnd(cline, sessionId, timeoutMs = 90_000) {
  const types = [];
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      unsub();
      resolve({ done: false, types, timeout: true });
    }, timeoutMs);
    const unsub = cline.subscribe(
      (event) => {
        types.push(event.type === 'agent_event' ? `agent:${event.payload?.event?.type}` : event.type);
        if (event.type === 'ended' || (event.type === 'agent_event' && event.payload?.event?.type === 'done')) {
          clearTimeout(timer);
          unsub();
          resolve({ done: true, types, reason: event.payload?.reason ?? event.payload?.event?.reason });
        }
      },
      { sessionId },
    );
  });
}

const results = [];

for (const testCase of CASES) {
  const caseLog = path.join(OUT_ROOT, 's2', `${testCase.name}.mock.jsonl`);
  const egressLog = path.join(OUT_ROOT, 's2', `${testCase.name}.egress.jsonl`);
  const mock = startMock({ script: testCase.script, logPath: caseLog, name: `s2-${testCase.name}` });
  const { baseUrl } = await mock.ready;
  const egress = makeEgressFetch(egressLog);
  const fetchImpl = testCase.blockFetch
    ? async (input) => {
        const url = typeof input === 'string' ? input : input.url;
        egress.calls.push({ at: new Date().toISOString(), url, blocked: true });
        throw new Error('privacy_filter_unavailable (mock fail-closed)');
      }
    : egress;
  const row = { ...testCase, script: undefined, baseUrl, baseUrlUsed: `${baseUrl}${testCase.pathSuffix}` };

  try {
    const cline = await ClineCore.create({
      clientName: 'hds-a0-spike',
      backendMode: 'local',
      fetch: fetchImpl,
    });
    const started = await cline.start({
      config: {
        providerId: testCase.providerId,
        modelId: 'mock-model',
        apiKey: 'sk-mock-not-a-secret',
        baseUrl: row.baseUrlUsed,
        cwd: sandbox.dirs.cwd,
        workspaceRoot: sandbox.dirs.cwd,
        systemPrompt: 'Отвечай одним словом.',
        enableTools: false,
        enableSpawnAgent: false,
        enableAgentTeams: false,
        maxIterations: 2,
      },
    });
    const end = waitEnd(cline, started.sessionId);
    await cline.send({ sessionId: started.sessionId, prompt: 'скажи привет' });
    row.end = await end;
    await cline.stop(started.sessionId);
    await cline.dispose();
  } catch (error) {
    row.error = String(error?.message ?? error);
  } finally {
    await mock.stop();
  }

  const fsmod = await import('node:fs');
  const mockLogLines = fsmod.existsSync(caseLog)
    ? fsmod
        .readFileSync(caseLog, 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((l) => JSON.parse(l))
    : [];

  row.mockRequests = mockLogLines.length;
  row.mockUrls = [...new Set(mockLogLines.map((l) => l.url))];
  row.mockStreamFlags = mockLogLines.map((l) => l.stream);
  row.mockAuthSchemes = [...new Set(mockLogLines.map((l) => l.authScheme))];
  row.egressCalls = egress.calls.length;
  row.egressUrls = egress.calls.map((c) => c.url);
  // для fail-closed на mock не должно прийти НИ ОДНОГО запроса
  row.equal = testCase.blockFetch
    ? row.mockRequests === 0
    : row.mockRequests === row.egressCalls && row.mockRequests > 0;
  results.push(row);
  console.log(
    `${testCase.name}: mock=${row.mockRequests} fetch=${row.egressCalls} equal=${row.equal} urls=${row.mockUrls.join(',')}`,
  );
}

const summary = {
  cases: results,
  allEqual: results.every((r) => r.equal),
  totalMock: results.reduce((s, r) => s + r.mockRequests, 0),
  totalFetch: results.reduce((s, r) => s + r.egressCalls, 0),
};
const file = writeResult(path.join('s2', 'result.json'), summary);
console.log(`wrote ${file}`);
console.log(JSON.stringify({ allEqual: summary.allEqual, totalMock: summary.totalMock, totalFetch: summary.totalFetch }, null, 2));
