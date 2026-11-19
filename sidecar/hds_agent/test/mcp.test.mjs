// TR-3.6 / TR-3.10 е, з: MCP подключается файлом по CLINE_MCP_SETTINGS_PATH.
//
// 1. В tools запроса к модели и в turn_end.tools есть <server>__search_local_files
//    и <server>__leak, но нет <server>__ask_my_files (его вырезает beforeModel).
// 2. Аргументы MCP-вызова (mcp_out) и результат (mcp_in) проходят egress_check:
//    заглушка получила метку, к модели ушла метка, исходного значения нет.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import {
  WORKER,
  defaultHost,
  makeProject,
  provider,
  readStubLog,
  startWorker,
  writeStubMcp,
} from './fixtures/harness.mjs';
import { startMockProvider, toolNames } from './fixtures/mock_provider.mjs';

test('MCP: инструменты заглушки в запросе, ask_my_files вырезан', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  writeStubMcp(dirs);
  const mock = startMockProvider([{ text: 'готово' }]);
  const base = await mock.ready;
  const log = [];
  const worker = startWorker(dirs, defaultHost(log));
  t.after(async () => {
    await worker.stop();
    await mock.stop();
    fs.rmSync(dirs.root, { recursive: true, force: true });
  });

  const ready = await worker.call('hello', { protocol: 1 });
  assert.equal(ready.protocol, 1);
  const started = await worker.call(
    'start',
    { project: 'P0001', provider: provider(base), max_iterations: 4 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  assert.deepEqual(started.result.mcp_servers, ['stub'], 'серверы из файла настроек');

  const sent = await worker.call('send', { text: 'привет' });
  assert.equal(sent.ok, true);
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  const names = toolNames(mock.requests[0]);
  for (const expected of ['stub__search_local_files', 'stub__leak']) {
    assert.ok(names.includes(expected), `${expected} должен быть в tools запроса: ${names}`);
  }
  assert.ok(!names.includes('stub__ask_my_files'), 'ask_my_files вырезан из запроса');

  assert.ok(Array.isArray(end.tools), 'turn_end.tools — список');
  for (const expected of ['stub__search_local_files', 'stub__leak']) {
    assert.ok(end.tools.includes(expected), `${expected} в turn_end.tools`);
  }
  assert.ok(!end.tools.includes('stub__ask_my_files'), 'ask_my_files нет в turn_end.tools');
  assert.ok(!end.tools.includes('run_commands'), 'run_commands выключен политикой');
});

test('MCP: mcp_out отдаёт метку заглушке, mcp_in — метку модели', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const stubLog = writeStubMcp(dirs);
  const mock = startMockProvider([
    {
      toolCalls: [
        { name: 'stub__search_local_files', arguments: { query: 'найди Иванов Иван' } },
        { name: 'stub__leak', arguments: {} },
      ],
    },
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
  await worker.call('send', { text: 'поищи' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  assert.equal(mock.requests.length, 2, 'два запроса к модели');
  const calls = readStubLog(stubLog);
  const search = calls.find((c) => c.name === 'search_local_files');
  assert.ok(search, `заглушка получила вызов: ${JSON.stringify(calls)}`);
  assert.equal(search.args.query, 'найди [PERSON_1]', 'mcp_out заменил значение меткой');

  const second = mock.requests[1].raw;
  assert.ok(second.includes('[PERSON_1]'), 'во втором запросе метка');
  assert.ok(!second.includes('Иванов Иван'), 'исходное значение к модели не ушло');

  const kinds = log.filter((l) => l.call === 'egress_check').map((l) => l.payload?.kind);
  assert.ok(kinds.includes('mcp_out'), `есть egress_check mcp_out: ${kinds}`);
  assert.ok(kinds.includes('mcp_in'), `есть egress_check mcp_in: ${kinds}`);
});
