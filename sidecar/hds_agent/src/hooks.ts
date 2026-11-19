// Хуки сессии: hds-guard (beforeTool, TR-3.4/3.5) и фильтр MCP (mcp_out /
// mcp_in, TR-6.1). Хуки НИКОГДА не бросают: исключение в хуке валит весь
// прогон (A0 §3.3), поэтому любая ошибка превращается в {skip: true, reason}.
// Таймаута хуков в SDK 0.2.0 нет — свой таймаут на каждую проверку.

import path from 'node:path';

import { checkWithin, withTimeout } from './guard.ts';
import type { Channel, Json } from './rpc.ts';
import { ASK_USER, OFFICE_TOOLS } from './tools.ts';

export const GUARD_TIMEOUT_MS = 2_000;

/** Встроенные файловые инструменты, разрешённые политикой (П-4). */
export const FILE_TOOLS = ['read_files', 'search_codebase', 'editor'] as const;
/** Выключены политикой (D16, интернет — через tavily); guard страхует. */
export const DENIED_TOOLS = ['run_commands', 'fetch_web_content', 'apply_patch'] as const;

const OFFICE_EXT = new Set(['.docx', '.xlsx', '.pptx']);
const READONLY_IMAGE = /\.(pdf|pptx|mpp)\.md$/i;
const PATH_KEYS = new Set(['path', 'file_path', 'filePath', 'paths', 'cwd', 'directory', 'dir', 'root']);

export interface HookOptions {
  workspace: string;
  channel: Channel;
  subagents: boolean;
  /** Сообщить UI об отказе/срабатывании (событие воркера). */
  notify?: (event: Json) => void;
}

export function isMcpTool(name: string): boolean {
  return name.includes('__');
}

/** Все строки-пути из аргументов инструмента (до глубины 4). */
export function collectPaths(input: Json, depth = 0, out: string[] = []): string[] {
  if (depth > 4 || input === null || input === undefined) return out;
  if (typeof input === 'string') {
    if (depth === 0) out.push(input);
    return out;
  }
  if (Array.isArray(input)) {
    for (const item of input) {
      if (typeof item === 'string' && depth === 0) out.push(item);
      else collectPaths(item, depth + 1, out);
    }
    return out;
  }
  if (typeof input === 'object') {
    for (const [key, value] of Object.entries(input)) {
      if (PATH_KEYS.has(key)) {
        if (typeof value === 'string') out.push(value);
        else if (Array.isArray(value)) {
          for (const v of value) if (typeof v === 'string') out.push(v);
        }
      } else if (value && typeof value === 'object') {
        collectPaths(value, depth + 1, out);
      }
    }
  }
  return out;
}

/** Политика по виду файла: что инструменту нельзя делать с этим путём. */
export function formatReject(tool: string, filePath: string): string | null {
  const ext = path.extname(filePath).toLowerCase();
  if (tool === 'read_files' && OFFICE_EXT.has(ext)) {
    return `для ${ext.slice(1)} используй office_read`;
  }
  if (tool === 'editor') {
    if (OFFICE_EXT.has(ext)) {
      return `editor не правит ${ext.slice(1)}: используй office_edit или office_create`;
    }
    if (READONLY_IMAGE.test(filePath)) {
      return 'образ документа только для чтения: правки оформляй новым документом';
    }
  }
  return null;
}

/** Решение guard по одному вызову инструмента (без MCP-фильтра). */
export async function guardDecision(
  tool: string,
  input: Json,
  opts: Pick<HookOptions, 'workspace' | 'subagents'>,
): Promise<{ skip: true; reason: string } | undefined> {
  if ((DENIED_TOOLS as readonly string[]).includes(tool)) {
    return { skip: true, reason: `hds-guard: инструмент ${tool} недоступен в анонимной зоне` };
  }
  if (isMcpTool(tool) || tool === ASK_USER) return undefined;
  const isFileTool = (FILE_TOOLS as readonly string[]).includes(tool);
  const isOffice = (OFFICE_TOOLS as readonly string[]).includes(tool);
  if (!isFileTool && !isOffice) {
    if (opts.subagents && /agent/i.test(tool)) return undefined;
    return { skip: true, reason: `hds-guard: инструмент ${tool} не разрешён` };
  }
  const paths = collectPaths(input);
  if (paths.length === 0 && (tool === 'read_files' || tool === 'editor' || isOffice)) {
    return { skip: true, reason: 'hds-guard: не указан путь' };
  }
  for (const candidate of paths) {
    const check = await checkWithin(opts.workspace, candidate);
    if (!check.ok) return { skip: true, reason: `hds-guard: ${check.reason}` };
    const policy = formatReject(tool, check.resolved);
    if (policy) return { skip: true, reason: `hds-guard: ${policy}` };
  }
  return undefined;
}

function errorText(error: unknown): string {
  return String((error as Error)?.message ?? error);
}

/** Собрать хуки для config.hooks. */
export function buildHooks(opts: HookOptions) {
  const { channel, notify } = opts;
  const deny = (tool: string, reason: string) => {
    notify?.({ kind: 'guard_denied', tool, reason });
    return { skip: true, reason };
  };

  const beforeTool = async (context: Json) => {
    try {
      const tool = String(context?.tool?.name ?? context?.toolCall?.toolName ?? '');
      const input = context?.input;
      const decision = await withTimeout(guardDecision(tool, input, opts), GUARD_TIMEOUT_MS, () => ({
        skip: true as const,
        reason: 'hds-guard: таймаут проверки пути',
      }));
      if (decision) return deny(tool, decision.reason);
      if (isMcpTool(tool)) {
        // mcp_out: аргументы внешнего MCP проходят словарь; секрет — блокировка.
        try {
          const checked = await channel.host('egress_check', { kind: 'mcp_out', body: input ?? {} });
          return { input: checked?.body ?? input };
        } catch (error) {
          return deny(tool, `privacy_filter_unavailable: ${errorText(error)}`);
        }
      }
      return undefined;
    } catch (error) {
      return { skip: true, reason: `hds-guard: ошибка проверки: ${errorText(error)}` };
    }
  };

  const afterTool = async (context: Json) => {
    try {
      const tool = String(context?.tool?.name ?? context?.toolCall?.toolName ?? '');
      if (!isMcpTool(tool)) return undefined;
      const result = context?.result ?? {};
      try {
        const checked = await channel.host('egress_check', {
          kind: 'mcp_in',
          body: result.output ?? null,
        });
        return { result: { ...result, output: checked?.body ?? null } };
      } catch (error) {
        // Результат, который нельзя проверить, модели не показываем.
        return {
          result: { output: `privacy_filter_unavailable: ${errorText(error)}`, isError: true },
        };
      }
    } catch {
      return undefined;
    }
  };

  return { beforeTool, afterTool };
}

