// TR-3.9 / TR-3.10: abort во время хода.
//
// Ход завершается turn_end с причиной aborted и ok=false; воркер жив и
// следующий send отрабатывает обычным образом.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test('abort: ход прерван, процесс жив', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const mock = startMockProvider([{ text: 'поздний ответ', delayMs: 5000 }, { text: 'готово' }]);
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

  await worker.call('send', { text: 'долгий запрос' });
  await sleep(700);
  const aborted = await worker.call('abort', {}, 30_000);
  assert.equal(aborted.ok, true, aborted.error);

  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 60_000);
  assert.equal(end.ok, false, 'прерванный ход не успешен');
  assert.equal(end.reason, 'aborted', `причина хода: ${end.reason} / ${end.error}`);

  await worker.call('send', { text: 'следующий запрос' });
  const next = await worker.waitEvent(
    (e) => e.kind === 'turn_end' && e.text === 'готово',
    60_000,
  );
  assert.equal(next.ok, true);
});
