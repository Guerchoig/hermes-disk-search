// Общий стенд спайков A0: песочница окружения, mock-провайдер, обёртки SDK.
//
// Важно: окружение (HOME/USERPROFILE/APPDATA/LOCALAPPDATA/TEMP) переопределяется
// ДО загрузки @cline/sdk — поэтому SDK здесь импортируется динамически, после
// applySandboxEnv(). Сценарий задаёт каталог песочницы через SANDBOX_ROOT
// (по умолчанию <out>/agent_a0/s1/home).

import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

/** Корень артефактов A0 (tools/parity/out/agent_a0) — вычисляется от места стенда. */
export const OUT_ROOT = process.env.A0_OUT_ROOT ?? path.resolve(HERE, '..');

/**
 * Переопределяет профильные переменные окружения так, чтобы ни один модуль SDK
 * не мог «случайно» попасть в реальный профиль пользователя.
 * Возвращает объект: какие переменные были до подмены и куда указывают теперь.
 */
export function applySandboxEnv(rootDir) {
  const root = path.resolve(rootDir);
  const dirs = {
    home: path.join(root, 'home'),
    appData: path.join(root, 'AppData', 'Roaming'),
    localAppData: path.join(root, 'AppData', 'Local'),
    temp: path.join(root, 'Temp'),
    cwd: path.join(root, 'workspace'),
    state: path.join(root, 'state'),
  };
  for (const dir of Object.values(dirs)) mkdirSync(dir, { recursive: true });

  const before = {
    USERPROFILE: process.env.USERPROFILE,
    HOME: process.env.HOME,
    APPDATA: process.env.APPDATA,
    LOCALAPPDATA: process.env.LOCALAPPDATA,
    TEMP: process.env.TEMP,
    TMP: process.env.TMP,
    HOMEDRIVE: process.env.HOMEDRIVE,
    HOMEPATH: process.env.HOMEPATH,
  };

  process.env.USERPROFILE = dirs.home;
  process.env.HOME = dirs.home;
  process.env.APPDATA = dirs.appData;
  process.env.LOCALAPPDATA = dirs.localAppData;
  process.env.TEMP = dirs.temp;
  process.env.TMP = dirs.temp;
  const drive = path.parse(dirs.home).root.replace(/\\$/, '');
  process.env.HOMEDRIVE = drive;
  process.env.HOMEPATH = dirs.home.slice(drive.length);

  return { root, dirs, before };
}

/** Поднимает mock-провайдер отдельным процессом, ждёт строку с адресом. */
export function startMock({ script, scriptPath, logPath, name = 'mock' } = {}) {
  let scriptFile = scriptPath;
  if (!scriptFile && Array.isArray(script)) {
    scriptFile = path.join(OUT_ROOT, `${name}_script.json`);
    mkdirSync(path.dirname(scriptFile), { recursive: true });
    writeFileSync(scriptFile, JSON.stringify(script, null, 2), 'utf8');
  }
  if (logPath) {
    mkdirSync(path.dirname(logPath), { recursive: true });
    // журнал прошлого прогона не должен попадать в счётчики текущего
    rmSync(logPath, { force: true });
  }
  const args = [path.join(HERE, 'mock_provider.mjs'), '--port', '0'];
  if (scriptFile) args.push('--script', scriptFile);
  if (logPath) args.push('--log', logPath);

  const child = spawn(process.execPath, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const ready = new Promise((resolve, reject) => {
    let buf = '';
    const timer = setTimeout(() => reject(new Error(`mock did not start: ${buf}`)), 15_000);
    child.stdout.on('data', (d) => {
      buf += d.toString();
      const m = buf.match(/http:\/\/127\.0\.0\.1:(\d+)/);
      if (m) {
        clearTimeout(timer);
        resolve({ port: Number(m[1]), baseUrl: `http://127.0.0.1:${m[1]}` });
      }
    });
    child.stderr.on('data', (d) => {
      buf += d.toString();
    });
    child.on('exit', (code) => reject(new Error(`mock exited (${code}): ${buf}`)));
  });
  return {
    child,
    scriptFile,
    ready,
    async stop() {
      // процесс мог уже завершиться (упал или убит) — иначе once('exit') не сработает
      if (child.exitCode !== null || child.signalCode !== null) return;
      const exited = new Promise((resolve) => child.once('exit', resolve));
      child.kill();
      await exited;
    },
  };
}

/** Счётчик запросов, пришедших на mock (независимая от нашего fetch проверка). */
export async function mockStats(baseUrl) {
  const res = await fetch(`${baseUrl}/__stats`);
  return res.json();
}

/**
 * Наш «подменённый» fetch: журналирует каждый вызов и проксирует в настоящий.
 * Именно такой объект hds передаст в ClineCore.create({ fetch }).
 */
export function makeEgressFetch(logFile) {
  const calls = [];

  // Headers/Request не перебираются через Object.keys — приводим к массиву имён
  const headerNames = (init, input) => {
    const source = init?.headers ?? (typeof input === 'object' ? input?.headers : undefined);
    try {
      return source ? [...new Headers(source).keys()] : [];
    } catch {
      return [];
    }
  };

  const fn = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const entry = {
      at: new Date().toISOString(),
      url,
      method: init?.method ?? (typeof input === 'object' && input?.method) ?? 'GET',
      headers: headerNames(init, input),
      bodyBytes: typeof init?.body === 'string' ? init.body.length : undefined,
    };
    calls.push(entry);
    if (logFile) {
      const { appendFileSync } = await import('node:fs');
      appendFileSync(logFile, `${JSON.stringify(entry)}\n`, 'utf8');
    }
    return fetch(input, init);
  };
  fn.calls = calls;
  return fn;
}

/** Ждёт завершения текущего прогона: агент шлёт события через subscribe. */
export function collectEvents(cline, { sessionId, timeoutMs = 120_000 } = {}) {
  const events = [];
  let finished;
  const done = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('run timeout')), timeoutMs);
    finished = (result) => {
      clearTimeout(timer);
      resolve(result);
    };
  });
  const unsub = cline.subscribe((event) => {
    events.push(event);
    if (event.type === 'run-finished' || event.type === 'run-failed') finished(event);
  }, sessionId ? { sessionId } : undefined);
  return { events, done, unsub };
}

/** Пишет JSON-артефакт спайка и возвращает путь. */
export function writeResult(name, data) {
  const file = path.join(OUT_ROOT, name);
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(data, null, 2), 'utf8');
  return file;
}

export function readJsonIfExists(file) {
  return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : undefined;
}

export function resetDir(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
}

/**
 * Один прогон сессии SDK против mock-провайдера: поднимает mock, стартует
 * сессию, отправляет реплику, ждёт завершения, собирает события и журнал mock.
 * Возвращает всё, что нужно для выводов спайка.
 */
export async function runSession(opts = {}) {
  const {
    name = 'case',
    script = [{ text: 'ok' }],
    providerId = 'openrouter',
    pathSuffix = '/v1',
    baseUrlOverride,
    cwd,
    systemPrompt = 'Отвечай одним словом.',
    prompt = 'скажи привет',
    hooks,
    hookErrorMode,
    toolPolicies,
    enableTools = false,
    enableSpawnAgent = false,
    enableAgentTeams = false,
    maxIterations = 4,
    extraTools,
    initialMessages,
    mode,
    sessionIdForConfig,
    logger,
    timeoutMs = 120_000,
  } = opts;

  const { ClineCore } = await import('@cline/sdk');
  const mockLog = path.join(OUT_ROOT, name, 'mock.jsonl');
  const egressLog = path.join(OUT_ROOT, name, 'egress.jsonl');
  const mock = startMock({ script, logPath: mockLog, name });
  const { baseUrl } = await mock.ready;
  const egress = makeEgressFetch(egressLog);
  const row = { name, baseUrl, baseUrlUsed: baseUrlOverride ?? `${baseUrl}${pathSuffix}`, errors: [] };
  const events = [];
  const types = [];
  let finishWait = () => {};

  try {
    const cline = await ClineCore.create({
      clientName: 'hds-a0-spike',
      backendMode: 'local',
      fetch: egress,
      ...(logger ? { logger } : {}),
    });

    const started = await cline.start({
      config: {
        providerId,
        modelId: 'mock-model',
        apiKey: 'sk-mock-not-a-secret',
        baseUrl: row.baseUrlUsed,
        cwd,
        workspaceRoot: cwd,
        systemPrompt,
        enableTools,
        enableSpawnAgent,
        enableAgentTeams,
        maxIterations,
        ...(mode ? { mode } : {}),
        ...(sessionIdForConfig ? { sessionId: sessionIdForConfig } : {}),
        ...(hooks ? { hooks } : {}),
        ...(hookErrorMode ? { hookErrorMode } : {}),
        ...(extraTools ? { extraTools } : {}),
      },
      ...(toolPolicies ? { toolPolicies } : {}),
      ...(initialMessages ? { initialMessages } : {}),
    });
    row.sessionId = started.sessionId;
    row.manifestPath = started.manifestPath;
    row.messagesPath = started.messagesPath;
    row.pluginIssues = started.pluginIssues;

    // Подписка и таймаут живут до первого терминального события; при ошибке
    // `send` их надо снять, иначе процесс висит до timeoutMs.
    const ended = new Promise((resolve) => {
      let timer;
      const unsub = cline.subscribe(
        (event) => {
          const copy = JSON.parse(JSON.stringify(event, (k, v) => (typeof v === 'function' ? undefined : v)));
          copy.type === 'agent_event'
            ? types.push(`agent:${event.payload?.event?.type}`)
            : types.push(event.type);
          if (event.type !== 'chunk') events.push(copy);
          if (event.type === 'ended') {
            cleanup();
            resolve({ done: true, reason: event.payload?.reason });
          }
        },
        { sessionId: started.sessionId },
      );
      const cleanup = () => {
        clearTimeout(timer);
        try {
          unsub();
        } catch {
          /* уже отписан */
        }
      };
      timer = setTimeout(() => {
        cleanup();
        resolve({ done: false, timeout: true });
      }, timeoutMs);
      finishWait = (value) => {
        cleanup();
        resolve(value);
      };
    });

    const result = await cline.send({ sessionId: started.sessionId, prompt });
    row.resultStatus = result?.status;
    row.resultOutput = result?.outputText;
    row.end = await ended;
    row.types = types;

    // что SDK считает историей сессии (для S6)
    try {
      row.messages = await cline.readMessages(started.sessionId);
    } catch (error) {
      row.readMessagesError = String(error?.message ?? error);
    }
    try {
      row.usage = await cline.getAccumulatedUsage(started.sessionId);
    } catch (error) {
      row.usageError = String(error?.message ?? error);
    }

    await cline.stop(started.sessionId);
    await cline.dispose();
  } catch (error) {
    row.errors.push(String(error?.stack ?? error));
    // снимаем висящую подписку/таймер, иначе процесс доживает до timeoutMs
    finishWait({ done: false, cancelled: true });
  } finally {
    await mock.stop();
  }

  const { existsSync: ex, readFileSync: rf } = await import('node:fs');
  row.mockRequests = ex(mockLog) ? rf(mockLog, 'utf8').split('\n').filter(Boolean).length : 0;
  row.mockLog = ex(mockLog)
    ? rf(mockLog, 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((l) => JSON.parse(l))
    : [];
  row.egressCalls = egress.calls.length;
  return { row, events, mockLogPath: mockLog };
}
