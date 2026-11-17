// Трассировка файловых и сетевых обращений для спайка A0/S1.
// Подключается через `node --import ./fs_trace.mjs scenario.mjs` — до загрузки SDK.
// Пишет JSONL в HDS_TRACE_LOG. Ничего не блокирует, только наблюдает.
//
// Важно: cwd и профиль процессы — подменённые (см. stand.mjs applySandboxEnv),
// поэтому любой путь под реальным %USERPROFILE% в этом журнале = утечка мимо HOME.

import fs from 'node:fs';
import fsp from 'node:fs/promises';
import net from 'node:net';
import dns from 'node:dns';

const logFile = process.env.HDS_TRACE_LOG;
if (logFile) {
  // native append через захваченную функцию — чтобы не поймать самих себя
  const appendNative = fs.appendFileSync.bind(fs);
  const write = (obj) => {
    try {
      appendNative(logFile, `${JSON.stringify({ at: Date.now(), ...obj })}\n`);
    } catch {
      /* трассировка не должна ронять прогон */
    }
  };

  const seen = new Set();
  const reportFs = (op, target) => {
    let p = target;
    if (p instanceof URL) p = p.pathname;
    if (Buffer.isBuffer(p)) p = p.toString();
    if (typeof p !== 'string') return;
    const key = `${op}|${p}`;
    if (seen.has(key)) return;
    seen.add(key);
    write({ kind: 'fs', op, path: p });
  };

  const wrapSync = (name) => {
    const original = fs[name];
    if (typeof original !== 'function') return;
    fs[name] = function traced(...args) {
      reportFs(name, args[0]);
      return original.apply(this, args);
    };
  };
  const wrapCallback = (name) => {
    const original = fs[name];
    if (typeof original !== 'function') return;
    fs[name] = function traced(...args) {
      reportFs(name, args[0]);
      return original.apply(this, args);
    };
  };
  const wrapPromise = (name) => {
    const original = fsp[name];
    if (typeof original !== 'function') return;
    fsp[name] = function traced(...args) {
      reportFs(`promises.${name}`, args[0]);
      return original.apply(this, args);
    };
  };

  for (const name of [
    'openSync',
    'open',
    'readFileSync',
    'readFile',
    'writeFileSync',
    'writeFile',
    'appendFileSync',
    'appendFile',
    'mkdirSync',
    'mkdir',
    'statSync',
    'stat',
    'lstatSync',
    'lstat',
    'readdirSync',
    'readdir',
    'existsSync',
    'accessSync',
    'access',
    'unlinkSync',
    'unlink',
    'rmSync',
    'rm',
    'renameSync',
    'rename',
    'copyFileSync',
    'copyFile',
    'createReadStream',
    'createWriteStream',
    'watch',
    'watchFile',
  ]) {
    wrapSync(name);
    wrapCallback(name);
  }
  for (const name of [
    'open',
    'readFile',
    'writeFile',
    'appendFile',
    'mkdir',
    'stat',
    'lstat',
    'readdir',
    'access',
    'unlink',
    'rm',
    'rename',
    'copyFile',
  ]) {
    wrapPromise(name);
  }

  const originalConnect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function tracedConnect(...args) {
    try {
      let host;
      let port;
      if (typeof args[0] === 'object' && args[0] !== null) {
        host = args[0].host;
        port = args[0].port;
        if (args[0].path) write({ kind: 'net', op: 'connect', path: String(args[0].path) });
      } else if (typeof args[0] === 'number') {
        port = args[0];
        host = typeof args[1] === 'string' ? args[1] : '127.0.0.1';
      }
      if (port !== undefined) write({ kind: 'net', op: 'connect', host: host ?? 'localhost', port });
    } catch {
      /* ignore */
    }
    return originalConnect.apply(this, args);
  };

  const originalLookup = dns.lookup;
  dns.lookup = function tracedLookup(hostname, ...rest) {
    write({ kind: 'net', op: 'dns.lookup', host: String(hostname) });
    return originalLookup.call(this, hostname, ...rest);
  };

  const originalFetch = globalThis.fetch;
  globalThis.fetch = function tracedFetch(input, init) {
    try {
      const url =
        typeof input === 'string' ? input : input instanceof URL ? input.toString() : input?.url;
      write({ kind: 'net', op: 'fetch', url: String(url) });
    } catch {
      /* ignore */
    }
    return originalFetch.call(this, input, init);
  };

  const originalHttpsRequest = (await import('node:https')).request;
  write({ kind: 'meta', op: 'trace-installed', httpsRequestPatched: typeof originalHttpsRequest === 'function' });
}
