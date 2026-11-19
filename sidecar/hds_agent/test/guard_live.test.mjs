// TR-3.4 / TR-3.10 а, ж: hds-guard в живом прогоне.
//
// Отказ по пути (..\outside, UNC, NUL) доходит до модели текстом «hds-guard: …»,
// содержимое outside не уходит провайдеру; относительный путь читается из
// workspace; для docx — «используй office_read», editor по образу *.pdf.md — отказ.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

import { WORKER, defaultHost, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

const OUTSIDE = 'OUTSIDE-SECRET-CONTENT';

test('hds-guard: отказы по путям и форматам видны модели', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  fs.writeFileSync(path.join(dirs.workspace, 'allowed.txt'), 'ALLOWED-42');
  fs.writeFileSync(path.join(dirs.workspace, 'report.docx'), 'DOCX-BYTES');
  fs.writeFileSync(path.join(dirs.workspace, 'scan.pdf.md'), '# скан\n');
  fs.writeFileSync(path.join(dirs.outside, 'secret.txt'), 'OUTSIDE-SECRET-CONTENT');

  const mock = startMockProvider([
    {
      toolCalls: [
        { name: 'read_files', arguments: { files: [{ path: '..\\outside\\secret.txt' }] } },
        { name: 'read_files', arguments: { files: [{ path: '\\\\server\\share\\x.txt' }] } },
        { name: 'read_files', arguments: { files: [{ path: 'NUL' }] } },
      ],
    },
    {
      toolCalls: [
        { name: 'read_files', arguments: { files: [{ path: 'allowed.txt' }] } },
        { name: 'read_files', arguments: { files: [{ path: 'report.docx' }] } },
        { name: 'editor', arguments: { path: 'scan.pdf.md', new_text: 'правка' } },
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
  await worker.call('send', { text: 'прочитай файлы' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);
  assert.equal(mock.requests.length, 3, 'три запроса: шаги сценария');

  const afterDenied = mock.requests[1].raw;
  assert.ok(afterDenied.includes('hds-guard'), 'отказ по пути дошёл до модели');
  for (const reason of ['вне workspace', 'UNC', 'имя устройства']) {
    assert.ok(afterDenied.includes(reason), `причина «${reason}» в запросе: ${afterDenied.slice(0, 500)}`);
  }
  for (const request of mock.requests) {
    assert.ok(!request.raw.includes(OUTSIDE), 'содержимое outside не ушло провайдеру');
  }

  const afterAllowed = mock.requests[2].raw;
  assert.ok(afterAllowed.includes('ALLOWED-42'), 'относительный путь прочитан из workspace');
  assert.ok(afterAllowed.includes('используй office_read'), 'docx отправлен в office_read');
  assert.ok(afterAllowed.includes('только для чтения'), 'editor по *.pdf.md отклонён');

  const denied = worker.events.filter((e) => e.event?.kind === 'guard_denied');
  assert.ok(denied.length >= 5, `отказы видны событиями: ${denied.length}`);
});
