// TR-3.10 в: в окружении воркера нет ключей провайдеров и реального профиля.
//
// Стенд кладёт ключ провайдера в env родителя; harness (как crates/hds-agent/src/env.rs)
// его не передаёт. Проверяем по поведению: провайдер авторизуется только ключом из
// start, ключа родителя нет ни в одном исходящем запросе, а реальный профиль
// (<домашний каталог>\.cline) не упоминается ни в запросах, ни в событиях, ни в
// файлах состояния.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { WORKER, defaultHost, filesUnder, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

const PARENT_KEY = 'sk-live-PARENT-ENV-KEY-0123456789';

test('окружение: чужой ключ провайдера и реальный профиль не используются', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const saved = { OPENROUTER_API_KEY: process.env.OPENROUTER_API_KEY, HOME: process.env.HOME };
  process.env.OPENROUTER_API_KEY = PARENT_KEY;
  t.after(() => {
    if (saved.OPENROUTER_API_KEY === undefined) delete process.env.OPENROUTER_API_KEY;
    else process.env.OPENROUTER_API_KEY = saved.OPENROUTER_API_KEY;
    if (saved.HOME === undefined) delete process.env.HOME;
    else process.env.HOME = saved.HOME;
  });

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
  await worker.call('send', { text: 'привет' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  assert.equal(mock.requests.length, 1);
  // Ключ провайдера берётся только из start (harness provider() — sk-mock-not-a-secret).
  assert.equal(mock.requests[0].authorization, 'Bearer sk-mock-not-a-secret');

  const realProfile = path.join(os.homedir(), '.cline');
  const outgoing = mock.requests.map((r) => r.raw).join('\n');
  assert.ok(!outgoing.includes(PARENT_KEY), 'ключ родителя не ушёл провайдеру');
  assert.ok(!outgoing.includes(realProfile), 'реальный профиль не упомянут в запросе');

  const events = JSON.stringify(worker.events);
  assert.ok(!events.includes(PARENT_KEY), 'ключ родителя не попал в события');
  assert.ok(!events.includes(realProfile), 'реальный профиль не попал в события');
  assert.equal(worker.stderr().includes(PARENT_KEY), false, 'ключ родителя не попал в stderr');

  for (const file of [...filesUnder(dirs.state), ...filesUnder(dirs.config)]) {
    assert.ok(!file.text.includes(PARENT_KEY), `ключ родителя не должен быть в ${file.path}`);
    assert.ok(!file.text.includes(realProfile), `реальный профиль не должен быть в ${file.path}`);
  }
});
