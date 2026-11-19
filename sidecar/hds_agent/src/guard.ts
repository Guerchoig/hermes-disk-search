// hds-guard — проверка путей и политика инструментов (TR-3.4, TR-3.5, TR-3.10 а/б).
//
// Зеркало crates/hds-agent/src/paths.rs: тот же набор правил, потому что хук
// beforeTool живёт в Node, а Rust проверяет пути `host_call office_*` повторно.
//
// Правила пути:
//  * пустой путь, NUL-байт — отказ;
//  * UNC (`\\server\share`, `//server/share`) — отказ;
//  * пространство устройств (`\\?\`, `\\.\`) — отказ;
//  * альтернативный поток (`file.txt:hidden`) — отказ;
//  * имя устройства (`CON`, `NUL`, `COM1`, `LPT1`, …) — отказ;
//  * относительный путь абсолютизируется от корня workspace, `..` разрешается;
//  * junction/symlink разрешается через realpath (ближайший существующий предок);
//  * результат обязан лежать внутри workspace.
//
// Функции НИКОГДА не бросают: любая ошибка — отказ с причиной (A0 §3.3:
// исключение в хуке валит весь прогон).

import { promises as fsp } from 'node:fs';
import path from 'node:path';

export type PathCheck = { ok: true; resolved: string } | { ok: false; reason: string };

const DEVICE_NAMES = new Set([
  'CON', 'PRN', 'AUX', 'NUL',
  'COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8', 'COM9',
  'LPT1', 'LPT2', 'LPT3', 'LPT4', 'LPT5', 'LPT6', 'LPT7', 'LPT8', 'LPT9',
]);

/** Имя без расширения и без потока — для проверки имён устройств. */
function fileStem(raw: string): string {
  const base = raw.split(/[\\/]/).pop() ?? raw;
  return (base.split(/[.:]/)[0] ?? base).replace(/[. ]+$/, '');
}

/** Снять префикс `\\?\` у результата realpath (бывает на длинных путях). */
function stripVerbatim(p: string): string {
  if (p.startsWith('\\\\?\\UNC\\')) return `\\\\${p.slice(8)}`;
  if (p.startsWith('\\\\?\\')) return p.slice(4);
  return p;
}

/** Синтаксические правила, без обращения к ФС. */
export function lexicalReject(candidate: unknown): string | null {
  if (typeof candidate !== 'string') return 'пустой путь';
  const raw = candidate.trim();
  if (!raw || raw.includes('\0')) return 'пустой путь';
  if (raw.startsWith('\\\\') || raw.startsWith('//')) {
    const lower = raw.toLowerCase();
    if (lower.startsWith('\\\\?\\') || lower.startsWith('\\\\.\\')) {
      return 'префикс пространства устройств запрещён';
    }
    return 'UNC-путь запрещён';
  }
  // Двоеточие не на позиции буквы диска — альтернативный поток данных.
  for (let i = 2; i < raw.length; i += 1) {
    if (raw[i] === ':') return 'альтернативный поток данных запрещён';
  }
  if (raw.indexOf(':') === 0) return 'альтернативный поток данных запрещён';
  if (DEVICE_NAMES.has(fileStem(raw).toUpperCase())) return 'имя устройства запрещено';
  return null;
}

/** realpath существующей части пути + лексический остаток. */
async function canonical(p: string): Promise<string> {
  try {
    return stripVerbatim(await fsp.realpath(p));
  } catch {
    /* путь не существует — канонизируем ближайшего предка */
  }
  const tail: string[] = [];
  let current = p;
  for (;;) {
    const parent = path.dirname(current);
    if (parent === current) return path.resolve(p);
    tail.unshift(path.basename(current));
    current = parent;
    try {
      const real = stripVerbatim(await fsp.realpath(current));
      return path.resolve(real, ...tail);
    } catch {
      /* поднимаемся выше */
    }
  }
}

/** Лежит ли child внутри parent (по компонентам, без учёта регистра). */
export function isWithin(child: string, parent: string): boolean {
  const norm = (p: string) =>
    path.resolve(p).split(/[\\/]+/).filter(Boolean).map((s) => s.toLowerCase());
  const c = norm(child);
  const r = norm(parent);
  return r.length > 0 && c.length >= r.length && r.every((part, i) => c[i] === part);
}

/** Полная проверка пути относительно корня workspace. */
export async function checkWithin(root: string, candidate: unknown): Promise<PathCheck> {
  try {
    const lexical = lexicalReject(candidate);
    if (lexical) return { ok: false, reason: lexical };
    const raw = (candidate as string).trim();
    const rootReal = await canonical(path.resolve(root));
    const absolute = path.isAbsolute(raw) ? path.resolve(raw) : path.resolve(root, raw);
    const real = await canonical(absolute);
    if (!isWithin(real, rootReal)) return { ok: false, reason: 'путь вне workspace проекта' };
    return { ok: true, resolved: real };
  } catch (error) {
    return { ok: false, reason: `проверка пути не удалась: ${String((error as Error)?.message ?? error)}` };
  }
}

/** Promise с таймаутом: по истечении — значение по умолчанию (П-3: таймаута хука нет). */
export function withTimeout<T>(work: Promise<T>, ms: number, fallback: () => T): Promise<T> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback()), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback());
      },
    );
  });
}
