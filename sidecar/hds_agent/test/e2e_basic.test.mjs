// Первый сквозной прогон воркера на mock-провайдере (A3.0):
// hello → start → send → read_files внутри workspace → ответ модели.
// Проверяется: SDK принимает конфигурацию воркера, каждый запрос к модели
// прошёл egress_check, во второй запрос (с результатом инструмента) ушла
// метка, а не значение; данные SDK — в agent-state, а не в agent-config.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

import { defaultHost, makeProject, provider, startWorker, WORKER } from './fixtures/harness.mjs';
import { startMockProvider, toolNames } from './fixtures/mock_provider.mjs';

test('worker: read_files turn on mock provider, labels only', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  fs.writeFileSync(path.join(dirs.workspace, 'allowed.txt'), 'ALLOWED-42 Иванов Иван');
  const mock = startMockProvider([
    { toolCalls: [{ name: 'read_files', arguments: { files: [{ path: 'allowed.txt' }] } }] },
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

  const ready = await worker.call('hello', { protocol: 1 });
  assert.equal(ready.type, 'ready');
  assert.equal(ready.protocol, 1);
  assert.equal(ready.sdk, '0.2.0');

  const started = await worker.call('start', { project: 'P0001', provider: provider(base), max_iterations: 4 }, 120_000);
  assert.equal(started.ok, true, started.error);
  assert.ok(started.result.session_id);

  const sent = await worker.call('send', { text: 'прочитай allowed.txt' });
  assert.equal(sent.ok, true);
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);
  assert.equal(end.text, 'готово');

  assert.equal(mock.requests.length, 2, 'ровно два запроса к модели');
  const egress = log.filter((l) => l.call === 'egress_check' && l.payload?.kind === 'model');
  assert.equal(egress.length, mock.requests.length, 'каждый запрос прошёл egress_check');
  const names = toolNames(mock.requests[0]);
  for (const forbidden of ['run_commands', 'fetch_web_content']) {
    assert.ok(!names.includes(forbidden), `${forbidden} не должен быть в tools`);
  }
  for (const expected of ['read_files', 'office_read', 'ask_user']) {
    assert.ok(names.includes(expected), `${expected} должен быть в tools`);
  }
  assert.ok(mock.requests[1].raw.includes('[PERSON_1]'), 'метка ушла к модели');
  assert.ok(!mock.requests[1].raw.includes('Иванов Иван'), 'значение не ушло к модели');

  const kinds = worker.events.map((e) => e.event?.kind);
  for (const kind of ['tool_use', 'tool_result', 'text', 'agent_done', 'turn_end']) {
    assert.ok(kinds.includes(kind), `событие ${kind}`);
  }
  assert.ok(fs.existsSync(path.join(dirs.state, '.cline', 'data', 'sessions')), 'сессии в agent-state');
  assert.ok(!fs.existsSync(path.join(dirs.config, '.cline', 'data', 'sessions')), 'не в agent-config');
});
