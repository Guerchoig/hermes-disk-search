// TR-3.10 д: повторы запросов (429) идут через egressFetch.
//
// Число запросов mock равно числу egress_check kind=model: ни один повтор не
// уходит в обход фильтра. Первый ответ — 429, ход заканчивается успешно.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

test('429: каждый повтор проходит egress_check', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const mock = startMockProvider([{ status: 429 }, { text: 'готово' }]);
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
  await worker.call('send', { text: 'привет' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);

  const checks = log.filter((l) => l.call === 'egress_check' && l.payload?.kind === 'model');
  assert.ok(mock.requests.length >= 2, `повтор после 429 состоялся: ${mock.requests.length}`);
  assert.equal(checks.length, mock.requests.length, 'каждый запрос (включая повтор) прошёл egress_check');
  assert.equal(end.ok, true, `ход завершился успешно после 429: ${end.error ?? ''}`);
  assert.equal(end.text, 'готово');
});
