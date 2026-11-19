// TR-3.9 (возобновление) / TR-3.10: start с history и тем же session_id даёт
// модели прошлую переписку; ключ провайдера не оседает в файлах состояния.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

import { WORKER, defaultHost, filesUnder, makeProject, provider, startWorker } from './fixtures/harness.mjs';
import { startMockProvider } from './fixtures/mock_provider.mjs';

const PARENT_KEY = 'sk-live-PARENT-ENV-KEY-0123456789';
const START_KEY = 'sk-mock-not-a-secret';

function readMessages(state, sessionId) {
  const file = path.join(state, '.cline', 'data', 'sessions', sessionId, `${sessionId}.messages.json`);
  assert.ok(fs.existsSync(file), `файл истории: ${file}`);
  const parsed = JSON.parse(fs.readFileSync(file, 'utf8'));
  const messages = Array.isArray(parsed) ? parsed : parsed.messages;
  assert.ok(Array.isArray(messages) && messages.length > 0, 'история не пуста');
  return messages;
}

test('history: прошлая реплика доходит до модели, ключа нет в файлах', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  process.env.OPENROUTER_API_KEY = PARENT_KEY;
  t.after(() => {
    delete process.env.OPENROUTER_API_KEY;
  });

  const firstMock = startMockProvider([{ text: 'ЗАПОМНИЛ-ЭТО' }]);
  const firstBase = await firstMock.ready;
  const first = startWorker(dirs, defaultHost([]));
  let sessionId = '';
  let history = [];
  try {
    await first.call('hello', { protocol: 1 });
    const started = await first.call(
      'start',
      { project: 'P0001', provider: provider(firstBase), max_iterations: 4 },
      120_000,
    );
    assert.equal(started.ok, true, started.error);
    sessionId = started.result.session_id;
    await first.call('send', { text: 'первый вопрос' });
    const end = await first.waitEvent((e) => e.kind === 'turn_end', 120_000);
    assert.equal(end.ok, true, end.error);
    history = readMessages(dirs.state, sessionId);
  } finally {
    await first.stop();
    await firstMock.stop();
  }

  const secondMock = startMockProvider([{ text: 'продолжаем' }]);
  const secondBase = await secondMock.ready;
  const second = startWorker(dirs, defaultHost([]));
  t.after(async () => {
    await second.stop();
    await secondMock.stop();
    fs.rmSync(dirs.root, { recursive: true, force: true });
  });

  await second.call('hello', { protocol: 1 });
  const resumed = await second.call(
    'start',
    { project: 'P0001', provider: provider(secondBase), session_id: sessionId, history, max_iterations: 4 },
    120_000,
  );
  assert.equal(resumed.ok, true, resumed.error);
  assert.equal(resumed.result.session_id, sessionId, 'сессия возобновлена с тем же id');
  await second.call('send', { text: 'второй вопрос' });
  const end = await second.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  assert.equal(secondMock.requests.length, 1, 'первый запрос — сразу с историей');
  const request = secondMock.requests[0].raw;
  assert.ok(request.includes('ЗАПОМНИЛ-ЭТО'), 'прошлая реплика в первом запросе');
  assert.ok(request.includes('второй вопрос'), 'новая реплика тоже в запросе');

  for (const file of [...filesUnder(dirs.state), ...filesUnder(dirs.config)]) {
    assert.ok(!file.text.includes(START_KEY), `api_key провайдера не должен быть в ${file.path}`);
    assert.ok(!file.text.includes(PARENT_KEY), `ключ env родителя не должен быть в ${file.path}`);
  }
});
