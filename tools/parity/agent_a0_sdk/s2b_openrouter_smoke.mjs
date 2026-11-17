// S2b — дымовой прогон на **реальном** OpenRouter (согласие владельца, В-2).
//
// Ключ берётся из настроек владельца (env → ~/.cline/data/secrets.json →
// providers.json) и НИКУДА не пишется: ни в файлы, ни в вывод. В артефакт
// сохраняются только статус, длина ответа, usage и число вызовов fetch.
//
// Запуск: node s2b_openrouter_smoke.mjs
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { applySandboxEnv, makeEgressFetch, OUT_ROOT, writeResult } from './stand.mjs';

const BASE = path.join(OUT_ROOT, 's2b');
const sandbox = applySandboxEnv(path.join(BASE, 'sandbox'));
const realHome = sandbox.before.USERPROFILE;

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

/** Ключ и модель — только в память процесса. */
function resolveCredentials() {
  let apiKey = process.env.OPENROUTER_API_KEY ?? null;
  let model = process.env.A0_OPENROUTER_MODEL ?? null;

  const secrets = readJson(path.join(realHome, '.cline', 'data', 'secrets.json'));
  if (!apiKey && secrets?.openRouterApiKey) apiKey = secrets.openRouterApiKey;

  const providers = readJson(path.join(realHome, '.cline', 'data', 'settings', 'providers.json'));
  const entry = providers?.providers?.openrouter?.settings;
  if (!apiKey && entry?.apiKey) apiKey = entry.apiKey;
  if (!model && entry?.model) model = entry.model;

  return { apiKey, model: model ?? 'deepseek/deepseek-chat' };
}

const { apiKey, model } = resolveCredentials();
if (!apiKey) {
  console.error('нет ключа OpenRouter (env/secrets.json/providers.json) — прогон пропущен');
  process.exit(2);
}

const { ClineCore } = await import('@cline/sdk');
const egress = makeEgressFetch(path.join(BASE, 'egress.jsonl'));
const result = {
  model,
  baseUrl: 'https://openrouter.ai/api/v1',
  keySource: process.env.OPENROUTER_API_KEY ? 'env' : 'owner settings (memory only)',
  startedAt: new Date().toISOString(),
};

try {
  const cline = await ClineCore.create({
    clientName: 'hds-a0-spike',
    backendMode: 'local',
    fetch: egress,
  });

  const started = await cline.start({
    config: {
      providerId: 'openrouter',
      modelId: model,
      apiKey,
      baseUrl: result.baseUrl,
      cwd: sandbox.dirs.cwd,
      workspaceRoot: sandbox.dirs.cwd,
      systemPrompt: 'Отвечай одной короткой фразой.',
      enableTools: false,
      enableSpawnAgent: false,
      enableAgentTeams: false,
      maxIterations: 1,
      maxTokensPerTurn: 64,
    },
  });

  const t0 = Date.now();
  const ended = new Promise((resolve) => {
    const timer = setTimeout(() => {
      unsub();
      resolve({ done: false, timeout: true });
    }, 120_000);
    const unsub = cline.subscribe(
      (event) => {
        if (event.type === 'ended') {
          clearTimeout(timer);
          unsub();
          resolve({ done: true, reason: event.payload?.reason });
        }
      },
      { sessionId: started.sessionId },
    );
  });

  await cline.send({ sessionId: started.sessionId, prompt: 'скажи привет' });
  result.end = await ended;
  result.durationMs = Date.now() - t0;

  const messages = await cline.readMessages(started.sessionId);
  const assistantText = (messages ?? [])
    .filter((m) => m.role === 'assistant')
    .flatMap((m) => m.content ?? [])
    .filter((part) => part.type === 'text')
    .map((part) => part.text)
    .join(' ');
  // текст ответа не сохраняем — только длину (правило «не копировать ответы»)
  result.answerLength = assistantText.length;
  result.usage = await cline.getAccumulatedUsage(started.sessionId);

  await cline.stop(started.sessionId);
  await cline.dispose();
} catch (error) {
  result.error = String(error?.message ?? error).slice(0, 500);
} finally {
  result.egressCalls = egress.calls.length;
  result.egressUrls = egress.calls.map((c) => c.url);
}

const file = writeResult(path.join('s2b', 'result.json'), result);
console.log(`wrote ${file}`);
console.log(
  JSON.stringify(
    {
      model: result.model,
      end: result.end,
      durationMs: result.durationMs,
      answerLength: result.answerLength,
      egressCalls: result.egressCalls,
      error: result.error ?? null,
    },
    null,
    2,
  ),
);
