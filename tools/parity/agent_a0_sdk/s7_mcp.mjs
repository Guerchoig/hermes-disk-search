// S7 — MCP из ядра SDK 0.2.0: где лежит список серверов, как он попадает в
// сессию, видны ли инструменты модели, работают ли вызовы и хуки на MCP.
//
// Этап A: собственный stdio MCP (mcp_echo_server.mjs) — без ключей и сети.
// Этапы B/C/D (tavily, 1c-buddy, disk-search) — в s7_remote.mjs.
import fs from 'node:fs';
import path from 'node:path';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult } from './stand.mjs';

const ROOT = path.join(OUT_ROOT, 's7');
const sandbox = applySandboxEnv(path.join(ROOT, 'sandbox'));
const cwd = sandbox.dirs.cwd;
process.chdir(cwd);

const logLines = [];
const logger = {
  debug: (...a) => logLines.push(['debug', a.map(String).join(' ')]),
  info: (...a) => logLines.push(['info', a.map(String).join(' ')]),
  warn: (...a) => logLines.push(['warn', a.map(String).join(' ')]),
  error: (...a) => logLines.push(['error', a.map(String).join(' ')]),
  log: (...a) => logLines.push(['log', a.map(String).join(' ')]),
};

const settingsPath = path.join(
  sandbox.dirs.home,
  '.cline',
  'data',
  'settings',
  'cline_mcp_settings.json',
);
fs.mkdirSync(path.dirname(settingsPath), { recursive: true });

const echoServer = path.join(path.dirname(new URL(import.meta.url).pathname.slice(1)), 'mcp_echo_server.mjs');

// Находка: stdio-транспорт НЕ квотирует command — путь с пробелом
// ("C:\Program Files\nodejs\node.exe") разваливается на аргументы.
// Проверяем два обхода: (1) копия node.exe в путь без пробелов,
// (2) command в кавычках.
const binDir = path.join(ROOT, 'bin');
fs.mkdirSync(binDir, { recursive: true });
const nodeCopy = path.join(binDir, 'node.exe');
if (!fs.existsSync(nodeCopy)) fs.copyFileSync(process.execPath, nodeCopy);

const variants = [
  { name: 'copied-node', command: nodeCopy, args: [echoServer] },
  { name: 'quoted-node', command: `"${process.execPath}"`, args: [echoServer] },
  { name: 'bare-node', command: 'node', args: [echoServer] },
];

function writeSettings(command, args) {
  const settings = {
    mcpServers: {
      echo: {
        transport: { type: 'stdio', command, args },
        disabled: false,
        timeoutSeconds: 30,
      },
    },
  };
  fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2), 'utf8');
  return settings;
}

// A1: перебор вариантов запуска — что видит модель
const result = { settingsPath, variants: [], stages: [] };
let workingTool = null;

for (const variant of variants) {
  const settings = writeSettings(variant.command, variant.args);
  logLines.length = 0;
  const a1 = await runSession({
    name: `s7/a1-${variant.name}`,
    script: [{ text: 'ok' }],
    cwd,
    enableTools: true,
    prompt: 'какие инструменты доступны?',
    logger,
  });
  const first = a1.row.mockLog[0] ?? {};
  const mcpTools = (first.toolNames ?? []).filter((n) => n.includes('echo') && n !== 'read_files');
  result.variants.push({
    variant: variant.name,
    settings,
    toolNames: first.toolNames,
    mcpTools,
    log: logLines.filter((l) => l[1].includes('[mcp]')).slice(0, 10),
  });
  if (mcpTools.length && !workingTool) workingTool = mcpTools[0];
  console.log(`${variant.name}: mcp tools = ${JSON.stringify(mcpTools)}`);
}

if (!workingTool) {
  writeResult(path.join('s7', 'result-a.json'), result);
  console.log('MCP tools not available in any variant');
  process.exit(0);
}

// дальше работаем с найденным рабочим вариантом
writeSettings(nodeCopy, [echoServer]);
const mcpToolName = workingTool;
result.workingTool = mcpToolName;
const a2 = await runSession({
  name: 's7/a2-call',
  script: [
    {
      toolCalls: [
        { name: mcpToolName ?? 'echo', arguments: { text: 'привет из агента' } },
      ],
    },
    { text: 'готово' },
  ],
  cwd,
  enableTools: true,
  prompt: 'вызови echo',
  hooks: {
    beforeTool: (context) => {
      if (context.tool?.name === 'read_files') return undefined;
      // всё, что не встроенное — «внешний MCP», проверяем, что хук видит вызов
      return { input: { ...(context.input ?? {}), text: 'PATCHED-BY-BEFORE-TOOL' } };
    },
  },
});
result.stages.push({
  stage: 'A2 call + beforeTool input override',
  toolNameUsed: mcpToolName,
  requests: a2.row.mockRequests,
  toolResultsSeen: (a2.row.mockLog.slice(1) ?? []).flatMap((l) => l.toolResultSnippets ?? []),
  errors: a2.row.errors,
  end: a2.row.end,
});

// A3: хук блокирует MCP-инструмент
const a3 = await runSession({
  name: 's7/a3-block',
  script: [
    { toolCalls: [{ name: mcpToolName ?? 'echo', arguments: { text: 'x' } }] },
    { text: 'готово' },
  ],
  cwd,
  enableTools: true,
  prompt: 'вызови echo',
  hooks: {
    beforeTool: (context) =>
      context.tool?.name === 'read_files'
        ? undefined
        : { skip: true, reason: 'hds-guard: внешние MCP в этом режиме запрещены' },
  },
});
result.stages.push({
  stage: 'A3 beforeTool skip',
  requests: a3.row.mockRequests,
  toolResultsSeen: (a3.row.mockLog.slice(1) ?? []).flatMap((l) => l.toolResultSnippets ?? []),
  errors: a3.row.errors,
  end: a3.row.end,
});

writeResult(path.join('s7', 'result-a.json'), result);
console.log(JSON.stringify(result.stages, null, 2).slice(0, 3000));
