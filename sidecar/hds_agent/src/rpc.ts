// NDJSON-канал §4.3 поверх stdio (зеркало crates/hds-agent/src/protocol.rs).
//
// Вниз (hds → воркер): hello, start, send, abort, stop, shutdown, host_result.
// Вверх (воркер → hds): ready, event, host_call, done, error.
//
// Одна строка — один JSON-объект. id встречных запросов воркера — с префиксом
// `h`, чтобы Rust не путал их со своими. stdout принадлежит протоколу: всё
// диагностическое (console.*) перенаправлено в stderr (см. redirectConsole).

import { createInterface } from 'node:readline';
import { format } from 'node:util';

export const PROTOCOL_VERSION = 1;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Json = any;

export interface DownMessage {
  type: string;
  id?: string;
  [key: string]: Json;
}

interface Waiter {
  resolve: (value: Json) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

/** Ошибка host_call: Rust ответил ok=false или вызов не дождался ответа. */
export class HostCallError extends Error {
  call: string;
  constructor(call: string, message: string) {
    super(message);
    this.name = 'HostCallError';
    this.call = call;
  }
}

/** console.log/info/debug/warn → stderr: stdout занят протоколом. */
export function redirectConsole(): void {
  const toErr = (...args: unknown[]) => {
    process.stderr.write(`${format(...args)}\n`);
  };
  console.log = toErr;
  console.info = toErr;
  console.debug = toErr;
  console.warn = toErr;
}

/** JSON без функций, циклов и с Error → {message}. */
export function toJsonSafe(value: unknown): Json {
  const seen = new WeakSet<object>();
  const text = JSON.stringify(value, (_key, v) => {
    if (typeof v === 'function' || typeof v === 'symbol') return undefined;
    if (typeof v === 'bigint') return Number(v);
    if (v instanceof Error) return { message: v.message, name: v.name };
    if (v && typeof v === 'object') {
      if (seen.has(v)) return undefined;
      seen.add(v);
    }
    return v;
  });
  return text === undefined ? null : JSON.parse(text);
}

export class Channel {
  private seq = 0;
  private readonly waiting = new Map<string, Waiter>();
  private closed = false;
  hostTimeoutMs: number;

  constructor(hostTimeoutMs = 60_000) {
    this.hostTimeoutMs = hostTimeoutMs;
  }

  /** Записать сообщение вверх одной строкой. */
  write(message: Json): void {
    if (this.closed) return;
    try {
      process.stdout.write(`${JSON.stringify(toJsonSafe(message))}\n`);
    } catch (error) {
      process.stderr.write(`[hds-agent] write failed: ${String(error)}\n`);
    }
  }

  ready(id: string | undefined, extra: Json): void {
    this.write({ type: 'ready', id, protocol: PROTOCOL_VERSION, ...extra });
  }

  done(id: string | undefined, ok: boolean, result: Json = {}, error?: string): void {
    this.write({ type: 'done', id, ok, result, ...(error ? { error } : {}) });
  }

  event(id: string | undefined, event: Json): void {
    this.write({ type: 'event', id: id ?? 'event', event });
  }

  error(message: string, stack?: string): void {
    this.write({ type: 'error', message, ...(stack ? { stack } : {}) });
  }

  /** Встречный запрос к hds. Отказ (ok=false, таймаут, закрытие) — исключение. */
  host(call: string, payload: Json, timeoutMs = this.hostTimeoutMs): Promise<Json> {
    if (this.closed) return Promise.reject(new HostCallError(call, 'канал закрыт'));
    this.seq += 1;
    const id = `h${this.seq}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting.delete(id);
        reject(new HostCallError(call, `host_call ${call}: таймаут ${timeoutMs} мс`));
      }, timeoutMs);
      this.waiting.set(id, { resolve, reject, timer });
      this.write({ type: 'host_call', id, call, payload });
    });
  }

  private settle(message: DownMessage): void {
    const id = String(message.id ?? '');
    const waiter = this.waiting.get(id);
    if (!waiter) return;
    this.waiting.delete(id);
    clearTimeout(waiter.timer);
    if (message.ok) waiter.resolve(message.result ?? null);
    else waiter.reject(new HostCallError(id, String(message.error ?? 'host_call отклонён')));
  }

  /** Читать stdin построчно; host_result разбирается здесь, остальное — в handler. */
  listen(handler: (message: DownMessage) => void, onClose: () => void): void {
    const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
    rl.on('line', (line) => {
      const text = line.trim();
      if (!text.startsWith('{')) return;
      let message: DownMessage;
      try {
        message = JSON.parse(text);
      } catch {
        return;
      }
      if (message.type === 'host_result') {
        this.settle(message);
        return;
      }
      try {
        handler(message);
      } catch (error) {
        this.done(message.id, false, {}, String((error as Error)?.message ?? error));
      }
    });
    rl.on('close', () => {
      this.close();
      onClose();
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const [id, waiter] of this.waiting) {
      clearTimeout(waiter.timer);
      waiter.reject(new HostCallError(id, 'канал закрыт'));
    }
    this.waiting.clear();
  }
}
