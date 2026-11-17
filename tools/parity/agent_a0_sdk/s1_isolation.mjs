// S1 — изоляция конфигурации SDK. СТОП-ТОЧКА.
//
// Проверяется:
//  1) SDK не читает и не пишет в реальный %USERPROFILE%\.cline и в каталог
//     расширения Cline (журнал fs-трассировки + снимки до/после в s1_run.ps1);
//  2) не ходит в сеть мимо провайдера: все fetch/dns/connect — только loopback;
//  3) каталоги SDK (сессии, настройки) создаются внутри подменённого HOME;
//  4) потомки процесса (hub-демон) — считает PowerShell-обёртка s1_run.ps1.
//
// Запуск: node --import ./fs_trace.mjs s1_isolation.mjs (HDS_TRACE_LOG задаёт обёртка).
import fs from 'node:fs';
import path from 'node:path';
import { applySandboxEnv, startMock, makeEgressFetch, OUT_ROOT, writeResult } from './stand.mjs';

const ART = path.join(OUT_ROOT, 's1');
const SANDBOX = path.join(ART, 'sandbox');
const realUserProfile = process.env.USERPROFILE;
const realAppData = process.env.APPDATA;
const repoRoot = process.env.A0_REPO_ROOT ?? path.resolve(OUT_ROOT, '..', '..', '..', '..');

// Чистая песочница: что появится внутри sandbox — то и создал SDK.
// Артефакты самого спайка (журналы, снимки) лежат рядом, в s1\ и не стираются.
fs.rmSync(SANDBOX, { recursive: true, force: true });
const sandbox = applySandboxEnv(SANDBOX);

// Baseline трассировки снимаем ДО импорта SDK: обращения при импорте модулей
// (opentelemetry и прочее) — тоже интересны, они часть «что SDK трогает».
const traceFile = process.env.HDS_TRACE_LOG;
const traceLinesBefore = traceFile && fs.existsSync(traceFile)
  ? fs.readFileSync(traceFile, 'utf8').split('\n').filter(Boolean).length
  : 0;

const { ClineCore, resolveDefaultMcpSettingsPath } = await import('@cline/sdk');

// Куда SDK резолвит cline_mcp_settings.json при подменённом HOME — должно быть
// внутри песочницы, иначе агент читал бы MCP владельца.
let mcpSettingsPath;
try {
  mcpSettingsPath = typeof resolveDefaultMcpSettingsPath === 'function'
    ? resolveDefaultMcpSettingsPath()
    : 'not exported';
} catch (error) {
  mcpSettingsPath = `error: ${error?.message ?? error}`;
}

function replacer(key, value) {
  if (typeof value === 'function') return undefined;
  if (typeof value === 'string' && value.length > 300) return `${value.slice(0, 300)}…`;
  return value;
}

const mockLog = path.join(ART, 'mock-requests.jsonl');
const egressLog = path.join(ART, 'egress.jsonl');
fs.mkdirSync(ART, { recursive: true });
for (const f of [mockLog, egressLog]) fs.rmSync(f, { force: true });
const mock = startMock({ script: [{ text: 'ответ мок-провайдера' }], logPath: mockLog, name: 's1' });
const { baseUrl } = await mock.ready;

const egress = makeEgressFetch(egressLog);
const events = [];
const result = {
  sandbox,
  baseUrl,
  realUserProfile,
  realAppData,
  repoRoot,
  mcpSettingsPath,
  events: [],
  errors: [],
};

try {
  const cline = await ClineCore.create({
    clientName: 'hds-a0-spike',
    backendMode: 'local',
    fetch: egress,
  });

  const started = await cline.start({
    config: {
      providerId: 'openrouter',
      modelId: 'mock-model',
      apiKey: 'sk-mock-not-a-secret',
      baseUrl: `${baseUrl}/v1`,
      cwd: sandbox.dirs.cwd,
      workspaceRoot: sandbox.dirs.cwd,
      systemPrompt: 'Тестовый агент анонимной зоны.',
      enableTools: false,
      enableSpawnAgent: false,
      enableAgentTeams: false,
      maxIterations: 2,
    },
  });
  result.sessionId = started.sessionId;
  result.manifestPath = started.manifestPath;
  result.messagesPath = started.messagesPath;

  const unsub = cline.subscribe((e) => events.push(JSON.parse(JSON.stringify(e, replacer))), {
    sessionId: started.sessionId,
  });
  await cline.send({ sessionId: started.sessionId, prompt: 'скажи привет' });
  await new Promise((r) => setTimeout(r, 1500));
  unsub();

  await cline.stop(started.sessionId);
  await cline.dispose();
} catch (error) {
  result.errors.push(String(error?.stack ?? error));
} finally {
  await mock.stop();
}

result.events = events.map((e) => e.type);
result.mockRequestCount = fs.existsSync(mockLog)
  ? fs.readFileSync(mockLog, 'utf8').split('\n').filter(Boolean).length
  : 0;
result.egressCalls = egress.calls;

// ---- разбор fs-трассировки -------------------------------------------------
// Классификация путей:
//   sandbox        — внутри песочницы (правильно);
//   sdk            — код SDK и его зависимостей (node_modules), репозиторий hds;
//   profileLeak    — под реальным %USERPROFILE% вне репозитория = утечка;
//   other          — всё прочее (системные пути, шрифты и т.п.).
const violations = { realClineHome: [], realAppData: [], externalNet: [], profileOutsideRepo: [] };
const summary = {
  fsEvents: 0,
  netEvents: 0,
  byKind: {},
  sandboxPaths: 0,
  sdkPaths: 0,
  otherPaths: 0,
  otherSamples: [],
};

if (traceFile && fs.existsSync(traceFile)) {
  const lines = fs
    .readFileSync(traceFile, 'utf8')
    .split('\n')
    .filter(Boolean)
    .slice(traceLinesBefore);
  const realClineHome = path.join(realUserProfile, '.cline').toLowerCase();
  const realAppDataLower = String(realAppData).toLowerCase();
  const realProfileLower = String(realUserProfile).toLowerCase();
  const sandboxLower = sandbox.root.toLowerCase();
  const repoLower = path.resolve(repoRoot).toLowerCase();

  // ESM-загрузчик сообщает пути как file:///C:/... — приводим к обычному виду
  const normPath = (p) => {
    let s = String(p);
    if (s.startsWith('file://')) s = s.replace(/^file:\/\//, '');
    if (/^\/[A-Za-z]:/.test(s)) s = s.slice(1);
    return s.replace(/\//g, '\\');
  };

  for (const line of lines) {
    let entry;
    try {
      entry = JSON.parse(line);
    } catch {
      continue;
    }
    if (entry.kind === 'fs') {
      summary.fsEvents += 1;
      const p = normPath(entry.path);
      const lower = p.toLowerCase();
      summary.byKind[entry.op] = (summary.byKind[entry.op] ?? 0) + 1;
      if (lower.startsWith(sandboxLower)) summary.sandboxPaths += 1;
      else if (lower.startsWith(repoLower)) summary.sdkPaths += 1;
      else {
        summary.otherPaths += 1;
        if (summary.otherSamples.length < 40) summary.otherSamples.push(p);
      }
      if (lower.startsWith(realClineHome)) violations.realClineHome.push(p);
      else if (lower.startsWith(realAppDataLower)) violations.realAppData.push(p);
      else if (lower.startsWith(realProfileLower) && !lower.startsWith(repoLower)) {
        violations.profileOutsideRepo.push(p);
      }
    } else if (entry.kind === 'net') {
      summary.netEvents += 1;
      const target = entry.url ?? `${entry.host ?? ''}:${entry.port ?? ''}`;
      const isLoopback =
        /127\.0\.0\.1|localhost|\[::1\]|^(?::\d+)?$/i.test(String(target)) ||
        ['127.0.0.1', 'localhost', '::1', undefined, ''].includes(entry.host);
      if (!isLoopback) violations.externalNet.push({ ...entry, target });
    }
  }
}
result.traceSummary = summary;
result.violations = violations;

// ---- что появилось в песочнице --------------------------------------------
function walk(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc;
  for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, item.name);
    if (item.isDirectory()) walk(full, acc);
    else {
      try {
        acc.push({ rel: path.relative(SANDBOX, full), size: fs.statSync(full).size });
      } catch {
        /* ignore */
      }
    }
  }
  return acc;
}
result.sandboxFiles = walk(SANDBOX).filter((f) => !f.rel.startsWith('fs-trace'));

result.ok =
  result.errors.length === 0 &&
  violations.realClineHome.length === 0 &&
  violations.realAppData.length === 0 &&
  violations.profileOutsideRepo.length === 0 &&
  violations.externalNet.length === 0 &&
  String(mcpSettingsPath).toLowerCase().startsWith(sandbox.root.toLowerCase());

const file = writeResult(path.join('s1', 'result.json'), result);
console.log(`wrote ${file}`);
console.log(
  JSON.stringify(
    {
      ok: result.ok,
      mcpSettingsPath,
      fsEvents: summary.fsEvents,
      sandboxPaths: summary.sandboxPaths,
      sdkPaths: summary.sdkPaths,
      otherPaths: summary.otherPaths,
      realClineTouches: violations.realClineHome.length,
      realAppDataTouches: violations.realAppData.length,
      profileOutsideRepo: violations.profileOutsideRepo.length,
      externalNet: violations.externalNet.length,
      mockRequests: result.mockRequestCount,
      errors: result.errors.length,
    },
    null,
    2,
  ),
);

