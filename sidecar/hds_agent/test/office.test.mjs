// TR-3.5 / TR-3.10: office_* и ask_user выполняются только через host_call.
//
// Путь уходит ровно таким, каким его дал агент, ops — массивом; результат
// возвращается модели. Путь вне workspace отклоняет hds-guard: host_call нет.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

test('office_* и ask_user: payload host_call и результат у модели', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  const op = { op: 'replace_text', find: 'а', replace: 'б' };
  const mock = startMockProvider([
    {
      toolCalls: [
        { name: 'office_read', arguments: { path: 'sub/report.docx' } },
        { name: 'office_edit', arguments: { path: 'sub/report.docx', ops: [op] } },
        { name: 'office_create', arguments: { path: 'new.docx', kind: 'docx', content: { markdown: '# Заголовок' } } },
        { name: 'ask_user', arguments: { question: 'Продолжить?' } },
        { name: 'office_read', arguments: { path: '..\\outside\\a.docx' } },
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
    { project: 'P0001', provider: provider(base), max_iterations: 6 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  await worker.call('send', { text: 'обработай документ' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  const calls = log.filter((l) => l.call !== 'egress_check');
  const reads = calls.filter((c) => c.call === 'office_read');
  assert.equal(reads.length, 1, 'host_call office_read — только для пути внутри workspace');
  assert.equal(reads[0].payload.path, 'sub/report.docx', 'путь как дал агент');
  const edits = calls.filter((c) => c.call === 'office_edit');
  assert.equal(edits.length, 1);
  assert.equal(edits[0].payload.path, 'sub/report.docx');
  assert.deepEqual(edits[0].payload.ops, [op], 'ops — массив операций');
  const creates = calls.filter((c) => c.call === 'office_create');
  assert.equal(creates.length, 1);
  assert.equal(creates[0].payload.path, 'new.docx');
  assert.equal(creates[0].payload.kind, 'docx');
  assert.deepEqual(creates[0].payload.content, { markdown: '# Заголовок' });
  const asks = calls.filter((c) => c.call === 'ask_user');
  assert.equal(asks.length, 1);
  assert.equal(asks[0].payload.question, 'Продолжить?');
  assert.ok(
    !calls.some((c) => String(c.payload?.path ?? '').includes('outside')),
    'host_call с путём вне workspace не вызывался',
  );

  const result = mock.requests[1].raw;
  assert.ok(result.includes('¶1 Отчёт report.docx'), 'результат office_read дошёл до модели');
  assert.match(result, /applied\\?"?:1/, 'результат office_edit дошёл до модели');
  assert.ok(result.includes('да'), 'ответ ask_user дошёл до модели');
});
