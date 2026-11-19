// TR-6.5 (fail-closed) / §6.3 карты: если фильтр недоступен, запрос к модели
// не уходит ни при одной попытке, а ход завершается понятной ошибкой.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

test('fail-closed: отказ egress_check model → 0 запросов к модели', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const mock = startMockProvider([{ text: 'ответа быть не должно' }]);
  const base = await mock.ready;
  const log = [];
  const host = defaultHost(log);
  const refusingHost = async (call, payload) => {
    if (call === 'egress_check' && payload?.kind === 'model') {
      log.push({ call, payload });
      throw new Error('privacy_filter_unavailable: нет снимка vault');
    }
    return host(call, payload);
  };
  const worker = startWorker(dirs, refusingHost);
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
  await worker.call('send', { text: 'привет' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);

  assert.equal(mock.requests.length, 0, 'запрос к провайдеру не состоялся');
  const checks = log.filter((l) => l.call === 'egress_check' && l.payload?.kind === 'model');
  assert.ok(checks.length > 0, 'фильтр был вызван');
  assert.equal(end.ok, false);
  assert.match(String(end.error ?? ''), /privacy_filter_unavailable/);
});
