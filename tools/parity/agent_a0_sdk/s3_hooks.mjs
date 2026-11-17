// S3 — хуки beforeTool / afterTool и fail-closed + прототип проверки путей TR-3.4.
//
// Каждый подпункт — отдельный прогон против mock-провайдера, чтобы видеть, что
// именно попало в следующий запрос к модели (mock пишет тела запросов).
import fs from 'node:fs';
import path from 'node:path';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult, resetDir } from './stand.mjs';

const ROOT = path.join(OUT_ROOT, 's3');
const sandbox = applySandboxEnv(path.join(ROOT, 'sandbox'));
const workspace = sandbox.dirs.cwd;
const outside = path.join(ROOT, 'outside');

resetDir(outside);
fs.writeFileSync(path.join(workspace, 'allowed.txt'), 'ALLOWED-CONTENT-42\n', 'utf8');
fs.writeFileSync(path.join(outside, 'secret.txt'), 'SECRET-OUTSIDE-99\n', 'utf8');
// ВАЖНО (находка S3): встроенные файловые инструменты 0.2.0 резолвят
// относительные пути от cwd ПРОЦЕССА, а не от config.cwd. Поэтому hds_agent
// должен запускаться с рабочим каталогом <P>\workspace — здесь имитируем это.
process.chdir(workspace);
// junction внутри workspace, ведущий наружу — классический обход
const junctionPath = path.join(workspace, 'link-out');
try {
  fs.rmSync(junctionPath, { force: true, recursive: true });
  fs.symlinkSync(outside, junctionPath, 'junction');
} catch (error) {
  console.error('junction failed:', error.message);
}

// ---- прототип проверки пути (TR-3.4) --------------------------------------
const DEVICE_NAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i;

function safeResolve(inputPath, workspaceRoot) {
  const reasons = [];
  if (typeof inputPath !== 'string' || !inputPath.trim()) return { ok: false, reason: 'empty path' };
  if (inputPath.includes('\0')) reasons.push('nul byte');
  if (/^\\\\[^\\]/.test(inputPath) || inputPath.startsWith('//')) reasons.push('UNC path');
  if (inputPath.startsWith('\\\\?\\') || inputPath.startsWith('\\\\.\\')) reasons.push('device namespace');
  // альтернативный поток данных: file.txt:stream
  if (/:[^\\/]+$/.test(inputPath)) reasons.push('alternate data stream');
  const base = path.basename(inputPath.replace(/[\\/]+$/, ''));
  if (DEVICE_NAMES.test(base)) reasons.push('device name');
  if (reasons.length) return { ok: false, reason: reasons.join('; ') };

  const absolute = path.resolve(workspaceRoot, inputPath);
  let real;
  try {
    real = fs.realpathSync.native(absolute);
  } catch {
    real = path.resolve(absolute);
  }
  const wsReal = fs.realpathSync.native(workspaceRoot);
  const rel = path.relative(wsReal, real);
  const inside = rel === '' || (!rel.startsWith('..') && !path.isAbsolute(rel));
  if (!inside) return { ok: false, reason: 'outside workspace', resolved: real };
  return { ok: true, resolved: real };
}

const pathMatrix = [
  ['relative inside', 'allowed.txt'],
  ['parent traversal', '..\\..\\outside\\secret.txt'],
  ['absolute outside', path.join(outside, 'secret.txt')],
  ['absolute inside', path.join(workspace, 'allowed.txt')],
  ['junction outside', 'link-out\\secret.txt'],
  ['UNC', '\\\\server\\share\\file.txt'],
  ['device path', '\\\\?\\C:\\Windows\\win.ini'],
  ['ads stream', 'allowed.txt:hidden'],
  ['device name', 'NUL'],
].map(([name, value]) => ({ name, path: value, ...safeResolve(value, workspace) }));

const results = { pathMatrix, cases: [] };
console.log('path matrix:');
for (const row of pathMatrix) console.log(`  ${row.name}: ok=${row.ok} ${row.reason ?? ''}`);

// ---- прогоны с хуками -----------------------------------------------------
async function runCase(testCase) {
  const { row } = await runSession({
    name: `s3/${testCase.name}`,
    script: testCase.script,
    cwd: workspace,
    prompt: testCase.prompt ?? 'выполни задачу',
    enableTools: true,
    hooks: testCase.hooks,
    hookErrorMode: testCase.hookErrorMode,
    maxIterations: 3,
    timeoutMs: 90_000,
  });
  // что модель видела на втором шаге (после вызова инструмента)
  const later = row.mockLog.slice(1);
  const seen = later.flatMap((l) => l.toolResultSnippets ?? []);
  results.cases.push({
    name: testCase.name,
    description: testCase.description,
    mockRequests: row.mockRequests,
    end: row.end,
    types: row.types,
    toolNames: row.mockLog[0]?.toolNames,
    toolResultsSeen: seen,
    errors: row.errors,
  });
  console.log(
    `${testCase.name}: requests=${row.mockRequests} end=${row.end?.reason} seen=${JSON.stringify(seen).slice(0, 220)}`,
  );
}

const readCall = (p) => [
  { toolCalls: [{ name: 'read_files', arguments: { files: [{ path: p }] } }] },
  { text: 'готово' },
];

await runCase({
  name: 'baseline',
  description: 'разрешённое чтение внутри workspace',
  script: readCall('allowed.txt'),
});

await runCase({
  name: 'block-outside',
  description: 'beforeTool возвращает skip для пути вне workspace',
  script: readCall('..\\..\\outside\\secret.txt'),
  hooks: {
    beforeTool: (context) => {
      for (const file of context.input?.files ?? []) {
        const check = safeResolve(file?.path, workspace);
        if (!check.ok) return { skip: true, reason: `hds-guard: ${check.reason}` };
      }
      return undefined;
    },
  },
});

await runCase({
  name: 'substitute-input',
  description: 'beforeTool подменяет input (несуществующий путь -> allowed.txt)',
  script: readCall('missing.txt'),
  hooks: {
    beforeTool: (context) =>
      context.tool?.name === 'read_files' ? { input: { files: [{ path: 'allowed.txt' }] } } : undefined,
  },
});

await runCase({
  name: 'replace-result',
  description: 'afterTool подменяет результат',
  script: readCall('allowed.txt'),
  hooks: {
    afterTool: () => ({ result: { output: { replaced: true, text: 'REDACTED-BY-AFTERTOOL' } } }),
  },
});

await runCase({
  name: 'hook-throws-ignore',
  description: 'хук бросает исключение при hookErrorMode=ignore (по умолчанию)',
  script: readCall('allowed.txt'),
  hooks: {
    beforeTool: () => {
      throw new Error('hook exploded');
    },
  },
  hookErrorMode: 'ignore',
});

await runCase({
  name: 'hook-throws-throw',
  description: 'хук бросает исключение при hookErrorMode=throw',
  script: readCall('allowed.txt'),
  hooks: {
    beforeTool: () => {
      throw new Error('hook exploded');
    },
  },
  hookErrorMode: 'throw',
});

await runCase({
  name: 'hook-slow-5s',
  description: 'beforeTool спит 5 с — есть ли таймаут хука',
  script: readCall('allowed.txt'),
  hooks: {
    beforeTool: async () => {
      const started = Date.now();
      await new Promise((r) => setTimeout(r, 5000));
      return { reason: `slept ${Date.now() - started}ms` };
    },
  },
});

writeResult(path.join('s3', 'result.json'), results);
console.log('wrote s3/result.json');

