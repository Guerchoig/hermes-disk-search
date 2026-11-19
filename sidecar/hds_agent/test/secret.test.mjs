// TR-6.1(г) / TR-3.10: секрет блокируется на egress_check.
//
// 1. Секрет в аргументах MCP (точка mcp_out) → вызов MCP не состоялся, причина
//    доходит до модели, прогон не падает.
// 2. Секрет в реплике пользователя (точка model) → к модели не ушло ни одного
//    запроса, turn_end.ok=false, процесс жив и следующий send работает.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import {
  SECRET,
  WORKER,
  defaultHost,
  makeProject,
  provider,
  readStubLog,
  startWorker,
  writeStubMcp,
} from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

test('секрет в аргументах MCP: вызов не состоялся', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const stubLog = writeStubMcp(dirs);
  const mock = startMockProvider([
    { toolCalls: [{ name: 'stub__search_local_files', arguments: { query: `ключ ${SECRET}` } }] },
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
    { project: 'P0001', provider: provider(base), max_iterations: 4 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  await worker.call('send', { text: 'найди по ключу' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);

  assert.deepEqual(readStubLog(stubLog), [], 'заглушка MCP не получила вызов');
  const denied = worker.events.find((e) => e.event?.kind === 'guard_denied');
  assert.ok(denied, 'отказ виден событием guard_denied');
  assert.match(denied.event.reason, /privacy_filter_unavailable/);
  assert.ok(!mock.requests.some((r) => r.raw.includes(SECRET)), 'секрет не ушёл провайдеру');
  assert.ok(end, 'ход завершился событием turn_end');
});

test('секрет в реплике: ни одного запроса к модели, воркер жив', { timeout: 180_000 }, async (t) => {
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
    { project: 'P0001', provider: provider(base), max_iterations: 4 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);

  await worker.call('send', { text: `вот мой ключ ${SECRET}` });
  const blocked = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(blocked.ok, false, 'ход с секретом не может быть успешным');
  assert.match(String(blocked.error ?? ''), /privacy_filter_unavailable/);
  assert.equal(mock.requests.length, 0, 'к модели не ушло ни одного запроса');
  assert.ok(
    log.some((l) => l.call === 'egress_check' && l.payload?.kind === 'model'),
    'модельная точка фильтра вызвана',
  );

  // Процесс жив: следующий ход с чистой репликой проходит.
  await worker.call('send', { text: 'теперь чистая реплика' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end' && e.ok === true, 120_000);
  assert.equal(end.text, 'готово');
  assert.equal(mock.requests.length, 1, 'второй ход дошёл до провайдера');
});
