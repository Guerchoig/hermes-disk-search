// S7, этапы B/C/D: реальные MCP — tavily, 1c-buddy, disk-search (streamable HTTP).
//
// Ключи берутся из настроек владельца и передаются ТОЛЬКО как переменные
// окружения дочерним процессам MCP; в файлы репозитория и в отчёт не пишутся.
// В результате сохраняются только имена инструментов, длины ответов и время.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult } from './stand.mjs';

const ROOT = path.join(OUT_ROOT, 's7');
const sandbox = applySandboxEnv(path.join(ROOT, 'sandbox'));
const cwd = sandbox.dirs.cwd;
process.chdir(cwd);

const settingsPath = path.join(sandbox.dirs.home, '.cline', 'data', 'settings', 'cline_mcp_settings.json');
fs.mkdirSync(path.dirname(settingsPath), { recursive: true });

const ownerHome = process.env.USERPROFILE; // уже подменён — реальный берём из sandbox.before
const realHome = sandbox.before.USERPROFILE;
const result = { stages: [], note: 'agent must never log secret values' };
const secrets = [];

// --- ключ Tavily из настроек владельца (значение не печатаем) ---------------
function readOwnerMcpSettings() {
  const ownerSettings = path.join(realHome, '.cline', 'data', 'settings', 'cline_mcp_settings.json');
  if (!fs.existsSync(ownerSettings)) return null;
  try {
    return JSON.parse(fs.readFileSync(ownerSettings, 'utf8'));
  } catch (error) {
    result.tavilySettingsError = String(error.message);
    return null;
  }
}

const ownerSettings = readOwnerMcpSettings();

let tavilyKey = process.env.TAVILY_API_KEY ?? process.env.TAVILY_KEY ?? null;
if (!tavilyKey && ownerSettings?.mcpServers) {
  for (const entry of Object.values(ownerSettings.mcpServers)) {
    const env = entry?.transport?.env ?? entry?.env ?? {};
    if (env.TAVILY_API_KEY) {
      tavilyKey = env.TAVILY_API_KEY;
      break;
    }
  }
}
if (tavilyKey) secrets.push(tavilyKey);

// --- токен disk-search: из headers настроек владельца -----------------------
let diskSearchToken = null;
let diskSearchUrl = 'http://127.0.0.1:8787/mcp';
for (const entry of Object.values(ownerSettings?.mcpServers ?? {})) {
  const auth = entry?.headers?.Authorization ?? entry?.headers?.authorization;
  if (typeof auth === 'string' && auth.length > 10) {
    diskSearchToken = auth.replace(/^Bearer\s+/i, '').trim();
    if (typeof entry.url === 'string') diskSearchUrl = entry.url;
    break;
  }
}
if (diskSearchToken) secrets.push(diskSearchToken);

const oncBuddyExe = path.join(realHome, '.cline', 'mcp', '1c-buddy', '.venv', 'Scripts', '1c-buddy-mcp.exe');
const oncBuddyEnv = path.join(realHome, '.cline', 'mcp', '1c-buddy', '.env');
const tavilyEntry = path.join(path.dirname(new URL(import.meta.url).pathname.slice(1)), 'node_modules', 'tavily-mcp', 'build', 'index.js');

result.availability = {
  tavilyKey: Boolean(tavilyKey),
  oncBuddyExe: fs.existsSync(oncBuddyExe),
  oncBuddyEnv: fs.existsSync(oncBuddyEnv),
  tavilyEntry: fs.existsSync(tavilyEntry),
  diskSearchToken: Boolean(diskSearchToken),
};

function writeSettings(servers) {
  fs.writeFileSync(settingsPath, JSON.stringify({ mcpServers: servers }, null, 2), 'utf8');
}

/** Один прогон: список инструментов и (опционально) вызов одного из них. */
async function stage({ name, servers, call, prompt }) {
  writeSettings(servers);
  const started = Date.now();
  const script = call
    ? [{ toolCalls: [call] }, { text: 'готово' }]
    : [{ text: 'ok' }];
  const run = await runSession({
    name: `s7/${name}`,
    script,
    cwd,
    enableTools: true,
    prompt: prompt ?? 'какие инструменты доступны?',
    timeoutMs: 180_000,
  });
  const first = run.row.mockLog[0] ?? {};
  const later = run.row.mockLog.slice(1);
  const row = {
    stage: name,
    durationMs: Date.now() - started,
    requests: run.row.mockRequests,
    allTools: first.toolNames,
    end: run.row.end,
    errors: run.row.errors,
    toolResults: later.flatMap((l) => l.toolResultSnippets ?? []).map((t) => ({
      length: t.length,
      head: t.slice(0, 160),
    })),
  };
  result.stages.push(row);
  console.log(`${name}: tools=${(first.toolNames ?? []).length} end=${run.row.end?.reason} results=${row.toolResults.length}`);
  return row;
}

// --- B. tavily -------------------------------------------------------------
if (tavilyKey) {
  const tavilyServer = {
    tavily: {
      transport: {
        type: 'stdio',
        command: 'node',
        args: [tavilyEntry],
        env: { TAVILY_API_KEY: tavilyKey },
      },
      timeoutSeconds: 90,
    },
  };
  const listRow = await stage({ name: 'b1-tavily-list', servers: tavilyServer });
  const searchTool = (listRow.allTools ?? []).find((n) => n.includes('search'));
  if (searchTool) {
    await stage({
      name: 'b2-tavily-call',
      servers: tavilyServer,
      call: { name: searchTool, arguments: { query: '1С:ERP 2.5 управление холдингом' } },
      prompt: 'найди в интернете сведения о 1С:ERP 2.5',
    });
  }
} else {
  result.stages.push({ stage: 'b-tavily', skipped: 'нет ключа Tavily в настройках владельца' });
}

// --- C. 1c-buddy -----------------------------------------------------------
if (result.availability.oncBuddyExe && result.availability.oncBuddyEnv) {
  const oncServer = {
    '1c-buddy': {
      transport: {
        type: 'stdio',
        command: oncBuddyExe,
        args: ['--env-file', oncBuddyEnv],
        env: { MCP_TOOL_CALL_MODE: 'direct', ONEC_AI_TIMEOUT: '60', ONEC_AI_UI_LANGUAGE: 'russian' },
      },
      timeoutSeconds: 120,
    },
  };
  const listRow = await stage({ name: 'c1-onc-buddy-list', servers: oncServer });
  const askTool = (listRow.allTools ?? []).find((n) => n.includes('ask'));
  if (askTool) {
    await stage({
      name: 'c2-onc-buddy-call',
      servers: oncServer,
      call: { name: askTool, arguments: { question: 'Какие возможности типовой 1С:ERP закрывают учёт по обособленным подразделениям?' } },
      prompt: 'спроси базу знаний 1С',
    });
  }
} else {
  result.stages.push({ stage: 'c-onc-buddy', skipped: 'не найден exe/env 1c-buddy' });
}

// --- D. disk-search (streamable HTTP) -------------------------------------
if (diskSearchToken) {
  const httpServer = {
    'disk-search': {
      transport: {
        type: 'streamableHttp',
        url: diskSearchUrl,
        headers: { Authorization: `Bearer ${diskSearchToken}` },
      },
      timeoutSeconds: 60,
    },
  };
  const listRow = await stage({ name: 'd1-disk-search-list', servers: httpServer });
  const statusTool = (listRow.allTools ?? []).find((n) => n.includes('index_status'));
  if (statusTool) {
    await stage({
      name: 'd2-disk-search-call',
      servers: httpServer,
      call: { name: statusTool, arguments: {} },
      prompt: 'проверь состояние индекса',
    });
  }
} else {
  result.stages.push({ stage: 'd-disk-search', skipped: 'нет токена disk-search в ~/.cline/mcp.json' });
}

// --- проверка «секреты не утекли в артефакты» ------------------------------
const filesToScan = [];
for (const stage of result.stages) {
  if (!stage.stage) continue;
}
const scanDirs = [path.join(OUT_ROOT, 's7')];
let leaks = 0;
for (const dir of scanDirs) {
  const walk = (d) => {
    if (!fs.existsSync(d)) return;
    for (const item of fs.readdirSync(d, { withFileTypes: true })) {
      const full = path.join(d, item.name);
      if (item.isDirectory()) walk(full);
      else filesToScan.push(full);
    }
  };
  walk(dir);
}
for (const file of filesToScan) {
  // файл настроек MCP обязан содержать заголовок с токеном — его не считаем утечкой
  if (path.basename(file) === 'cline_mcp_settings.json') continue;
  try {
    const text = fs.readFileSync(file, 'utf8');
    for (const secret of secrets) if (secret && text.includes(secret)) leaks += 1;
  } catch {
    /* бинарники не читаем */
  }
}
result.secretLeaksInArtifacts = leaks;
result.secretLeakScanFiles = filesToScan.length;

writeResult(path.join('s7', 'result-remote.json'), result);
console.log(JSON.stringify({ availability: result.availability, secretLeaksInArtifacts: leaks }, null, 2));

