// Стенд e2e: запускает dist/worker.mjs так же, как crates/hds-agent
// (очищенное окружение, HOME = agent-config, CLINE_DATA_DIR = agent-state,
// cwd = workspace), и играет роль Rust-стороны: отвечает на host_call.
//
// Фильтр здесь — упрощённый словарь (значение → метка, секрет → отказ):
// проверяется, что воркер пропускает через egress_check ВСЁ исходящее, а не
// качество словаря (его покрывают тесты crates/hds-agent/src/filter.rs).
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const PKG = path.resolve(HERE, '..', '..');
export const WORKER = path.join(PKG, 'dist', 'worker.mjs');
export const MCP_STUB = path.join(HERE, 'mcp_stub.mjs');

export const DICTIONARY = [['Иванов Иван', '[PERSON_1]'], ['ООО Ромашка', '[ORG_1]']];
export const SECRET = 'sk-live-SECRET-0123456789';

/** Каталоги проекта P как в анонимной зоне. */
export function makeProject() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'hds-agent-e2e-'));
  const dirs = {
    root,
    workspace: path.join(root, 'P0001', 'workspace'),
    config: path.join(root, 'P0001', 'agent-config'),
    state: path.join(root, 'P0001', 'agent-state'),
    outside: path.join(root, 'outside'),
  };
  for (const d of [dirs.workspace, dirs.outside, path.join(dirs.config, '.cline', 'data', 'settings')]) {
    fs.mkdirSync(d, { recursive: true });
  }
  fs.mkdirSync(path.join(dirs.config, 'rules'), { recursive: true });
  fs.writeFileSync(path.join(dirs.config, 'rules', 'privacy.md'), '# Правила\n\nМетки не раскрывать.\n');
  dirs.mcpSettings = path.join(dirs.config, '.cline', 'data', 'settings', 'cline_mcp_settings.json');
  return dirs;
}

/** `command` в MCP не квотируется SDK (A0 §3.7): путь без пробелов или `node` из PATH. */
export function nodeCommand() {
  return process.execPath.includes(' ') ? 'node' : process.execPath;
}

export function writeMcpSettings(dirs, servers) {
  fs.writeFileSync(dirs.mcpSettings, JSON.stringify({ mcpServers: servers }, null, 2));
}

/** hds-файл настроек с одной stdio-заглушкой `stub`; возвращает путь её журнала. */
export function writeStubMcp(dirs, env = {}) {
  const stubLog = path.join(dirs.root, 'stub-calls.jsonl');
  writeMcpSettings(dirs, {
    stub: {
      transport: {
        type: 'stdio',
        command: nodeCommand(),
        args: [MCP_STUB],
        env: { HDS_STUB_LOG: stubLog, ...env },
      },
      timeout: 60,
    },
  });
  return stubLog;
}

/** Журнал заглушки MCP: [{name, args}] — что она реально получила. */
export function readStubLog(file) {
  try {
    return fs
      .readFileSync(file, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch {
    return [];
  }
}

/** Все файлы каталога с текстом — для поиска утечек (api_key, реальный профиль). */
export function filesUnder(dir) {
  const out = [];
  const walk = (current) => {
    let entries = [];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile()) {
        let text = '';
        try {
          text = fs.readFileSync(full, 'utf8');
        } catch {
          text = '';
        }
        out.push({ path: full, text });
      }
    }
  };
  walk(dir);
  return out;
}

function redact(value, hits) {
  if (typeof value === 'string') {
    if (value.includes(SECRET)) throw new Error('privacy_filter_unavailable: секрет в исходящем');
    let out = value;
    for (const [from, to] of DICTIONARY) {
      if (out.includes(from)) {
        hits.push(to);
        out = out.split(from).join(to);
      }
    }
    return out;
  }
  if (Array.isArray(value)) return value.map((v) => redact(v, hits));
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, redact(v, hits)]));
  }
  return value;
}

/** Обработчик host_call по умолчанию (роль GatewayHost). */
export function defaultHost(log) {
  return async (call, payload) => {
    log.push({ call, payload });
    if (call === 'egress_check') {
      const hits = [];
      return { body: redact(payload?.body ?? null, hits), hits: hits.length };
    }
    if (call === 'office_read') {
      return { kind: 'docx', markdown: `¶1 Отчёт ${path.basename(payload.path)}`, applied: 0 };
    }
    if (call === 'office_edit') return { kind: 'docx', markdown: '', applied: payload.ops.length };
    if (call === 'office_create') return { kind: payload.kind, markdown: '', applied: 0 };
    if (call === 'ask_user') return { answer: 'да' };
    throw new Error(`unknown host_call ${call}`);
  };
}

export function provider(baseUrl) {
  return {
    kind: 'openai-compatible',
    base_url: `${baseUrl}/v1`,
    model: 'mock-model',
    api_key: 'sk-mock-not-a-secret',
  };
}

function childEnv(dirs, extra = {}) {
  return {
    PATH: process.env.PATH ?? '',
    SystemRoot: process.env.SystemRoot ?? 'C:\\Windows',
    ComSpec: process.env.ComSpec ?? '',
    PATHEXT: process.env.PATHEXT ?? '',
    HOME: dirs.config,
    USERPROFILE: dirs.config,
    APPDATA: path.join(dirs.config, 'AppData', 'Roaming'),
    LOCALAPPDATA: path.join(dirs.config, 'AppData', 'Local'),
    TEMP: path.join(dirs.config, 'Temp'),
    TMP: path.join(dirs.config, 'Temp'),
    CLINE_DATA_DIR: path.join(dirs.state, '.cline', 'data'),
    CLINE_MCP_SETTINGS_PATH: dirs.mcpSettings,
    ...extra,
  };
}

/** Запустить воркер и вернуть клиент протокола (роль crates/hds-agent/src/worker.rs). */
export function startWorker(dirs, host, extraEnv = {}) {
  const child = spawn(process.execPath, [WORKER], {
    cwd: dirs.workspace,
    env: childEnv(dirs, extraEnv),
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const events = [];
  const waiters = new Map();
  const eventWaiters = new Set();
  let stderr = '';
  let seq = 0;
  child.stderr.on('data', (d) => {
    stderr += d.toString();
  });
  const write = (msg) => child.stdin.write(`${JSON.stringify(msg)}\n`);
  createInterface({ input: child.stdout }).on('line', async (line) => {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.type === 'host_call') {
      try {
        write({ type: 'host_result', id: msg.id, ok: true, result: await host(msg.call, msg.payload) });
      } catch (error) {
        write({ type: 'host_result', id: msg.id, ok: false, result: null, error: String(error?.message ?? error) });
      }
      return;
    }
    if ((msg.type === 'ready' || msg.type === 'done') && waiters.has(msg.id)) {
      waiters.get(msg.id)(msg);
      waiters.delete(msg.id);
      return;
    }
    events.push(msg);
    for (const check of [...eventWaiters]) check();
  });
  const call = (type, fields = {}, timeoutMs = 60_000) => {
    seq += 1;
    const id = String(seq);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        waiters.delete(id);
        reject(new Error(`${type}: таймаут; stderr=${stderr.slice(-2000)}`));
      }, timeoutMs);
      waiters.set(id, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      write({ type, id, ...fields });
    });
  };
  const waitEvent = (predicate, timeoutMs = 90_000) =>
    new Promise((resolve, reject) => {
      let timer;
      const check = () => {
        const found = events.find((e) => e.type === 'event' && predicate(e.event ?? {}));
        if (!found) return;
        eventWaiters.delete(check);
        clearTimeout(timer);
        resolve(found.event);
      };
      timer = setTimeout(() => {
        eventWaiters.delete(check);
        reject(new Error(`событие не пришло; stderr=${stderr.slice(-2000)}`));
      }, timeoutMs);
      eventWaiters.add(check);
      check();
    });
  const stop = async () => {
    try {
      await call('shutdown', {}, 15_000);
    } catch {
      /* процесс мог уже выйти */
    }
    await new Promise((resolve) => {
      if (child.exitCode !== null) return resolve();
      const timer = setTimeout(() => {
        child.kill();
        resolve();
      }, 5_000);
      child.on('exit', () => {
        clearTimeout(timer);
        resolve();
      });
    });
  };
  return { child, call, events, waitEvent, stop, stderr: () => stderr };
}

