// TR-3.7 / TR-3.10 д: суб-агенты включаются только настройкой, и их запросы
// тоже идут через egress_check (фильтр видит каждый запрос к провайдеру).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider, toolNames } from './fixtures/mock_provider.mjs';

test('subagents=false: инструмента spawn_agent нет', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const mock = startMockProvider([{ text: 'готово' }]);
  const base = await mock.ready;
  const log = [];
  const worker = startWorker(dirs, defaultHost(log));
  t.after(async () => {
    await worker.stop();
    await mock.stop();
    fs.rmSync(dirs.root, { recursive: true, force: true });
  });

  await worker.call('hello', { protocol: 1 });
  const started = await worker.call(
    'start',
    { project: 'P0001', provider: provider(base), subagents: false, max_iterations: 4 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  await worker.call('send', { text: 'привет' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  assert.ok(!toolNames(mock.requests[0]).includes('spawn_agent'), 'spawn_agent выключен');
  assert.ok(!end.tools.includes('spawn_agent'), 'spawn_agent нет и в turn_end.tools');
});

test('subagents=true: spawn_agent есть, запрос суб-агента через egress_check', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const mock = startMockProvider([
    { toolCalls: [{ name: 'spawn_agent', arguments: { systemPrompt: 'ты субагент', task: 'ответь' } }] },
    { text: 'субагент ответил' },
    { text: 'готово' },
  ]);
  const base = await mock.ready;
  const log = [];
  const worker = startWorker(dirs, defaultHost(log));
  t.after(async () => {
    await worker.stop();
    await mock.stop();
    fs.rmSync(dirs.root, { recursive: true, force: true });
  });

  await worker.call('hello', { protocol: 1 });
  const started = await worker.call(
    'start',
    { project: 'P0001', provider: provider(base), subagents: true, max_iterations: 6 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  await worker.call('send', { text: 'делегируй' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  assert.ok(toolNames(mock.requests[0]).includes('spawn_agent'), 'spawn_agent включён настройкой');
  assert.ok(mock.requests.length >= 3, `запросы: родитель, суб-агент, финал (${mock.requests.length})`);
  const subagentRequest = mock.requests.find((r) => r.raw.includes('ты субагент'));
  assert.ok(subagentRequest, 'запрос суб-агента ушёл провайдеру');
  const checks = log.filter((l) => l.call === 'egress_check' && l.payload?.kind === 'model');
  assert.equal(checks.length, mock.requests.length, 'включая запрос суб-агента');
  assert.ok(
    worker.events.some((e) => e.event?.kind === 'tool_use' && e.event?.tool === 'spawn_agent'),
    'вызов суб-агента виден событием',
  );
});
