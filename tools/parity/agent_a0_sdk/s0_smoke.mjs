// S0 (дым): минимальный прогон SDK 0.2.0 против mock-провайдера.
// Проверяет, что базовая связка «ClineCore + openrouter + baseUrl на mock» работает,
// прежде чем строить на ней S1/S2.
import path from 'node:path';
import { applySandboxEnv, startMock, makeEgressFetch, OUT_ROOT, writeResult } from './stand.mjs';

const sandbox = applySandboxEnv(path.join(OUT_ROOT, 's1', 'home'));
const { ClineCore } = await import('@cline/sdk');

/** JSON.stringify-редьюсер: отбрасывает функции и обрезает слишком длинные строки. */
function replacer(key, value) {
  if (typeof value === 'function') return undefined;
  if (typeof value === 'string' && value.length > 500) return `${value.slice(0, 500)}…`;
  return value;
}

const logPath = path.join(OUT_ROOT, 's0', 'mock-requests.jsonl');
const egressLog = path.join(OUT_ROOT, 's0', 'egress.jsonl');
const mock = startMock({ script: [{ text: 'привет от mock' }], logPath, name: 's0' });
const { baseUrl } = await mock.ready;

const egress = makeEgressFetch(egressLog);
const events = [];
const summary = { baseUrl, ok: false, errors: [] };

try {
  const cline = await ClineCore.create({
    clientName: 'hds-a0-spike',
    backendMode: 'local',
    fetch: egress,
  });

  const started = await cline.start({
    config: {
      providerId: 'openrouter',
      modelId: 'mock-model',
      apiKey: 'sk-mock-not-a-secret',
      baseUrl: `${baseUrl}/v1`,
      cwd: sandbox.dirs.cwd,
      workspaceRoot: sandbox.dirs.cwd,
      systemPrompt: 'Ты тестовый агент. Отвечай одним словом.',
      enableTools: true,
      enableSpawnAgent: false,
      enableAgentTeams: false,
      maxIterations: 2,
    },
  });
  summary.sessionId = started.sessionId;
  summary.manifestPath = started.manifestPath;
  summary.messagesPath = started.messagesPath;
  summary.pluginIssues = started.pluginIssues;

  const unsub = cline.subscribe(
    (event) => events.push(JSON.parse(JSON.stringify(event, replacer))),
    { sessionId: started.sessionId },
  );

  const result = await cline.send({ sessionId: started.sessionId, prompt: 'скажи привет' });
  summary.resultStatus = result?.status;
  summary.outputText = result?.outputText;
  summary.usage = result?.usage;
  // даём событиям дойти (локальный runtime шлёт их после runTurn)
  await new Promise((r) => setTimeout(r, 1500));
  unsub();

  summary.events = events;
  summary.egressCalls = egress.calls;
  summary.mockStats = await (await fetch(`${baseUrl}/__stats`)).json();
  summary.ok = true;

  await cline.stop(started.sessionId);
  await cline.dispose();
} catch (error) {
  summary.errors.push(String(error?.stack ?? error));
  summary.events = events.map((e) => e.type);
  summary.egressCalls = egress.calls;
} finally {
  await mock.stop();
}

const file = writeResult(path.join('s0', 'smoke.json'), summary);
console.log(`wrote ${file}`);
console.log(JSON.stringify({ ok: summary.ok, status: summary.resultStatus, out: summary.outputText }, null, 2));
