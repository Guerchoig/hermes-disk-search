// Сессия агента поверх @cline/sdk (TR-3.2…TR-3.9).
//
// * ClineCore.create({backendMode: "local", fetch: egressFetch}) — один на
//   процесс; данные SDK — CLINE_DATA_DIR (<P>\agent-state), MCP — файл
//   CLINE_MCP_SETTINGS_PATH (его пишет hds);
// * toolPolicies: read_files, search_codebase, editor — да; run_commands,
//   fetch_web_content — нет (D16); плагины выключены (TR-3.2 п.4);
// * extraTools: office_* и ask_user через host_call;
// * hooks: hds-guard + фильтр MCP; beforeModel убирает запрещённые
//   инструменты из запроса (страховка поверх политики).

import { promises as fsp } from 'node:fs';
import path from 'node:path';

import { buildHooks, DENIED_TOOLS } from './hooks.ts';
import type { Channel, Json } from './rpc.ts';
import { buildToolSpecs } from './tools.ts';

/** Инструменты, которых в запросе к модели быть не должно (TR-4.4, D16). */
const FORBIDDEN_IN_REQUEST = (name: string) =>
  (DENIED_TOOLS as readonly string[]).includes(name) || isForbiddenToolName(name);

/**
 * Имена, которые beforeModel вырезает из запроса к модели.
 *
 * `ask_my_files` приходит с суффиксом `__ask_my_files`, но инструменты из
 * MCP-серверов Agent Plugin получают ещё и хеш в имени
 * (`hds-agent-plugin_agentdecoy__ask_my_files_3deb181b`) — поэтому проверка не
 * только по концу строки, но и по шаблону с суффиксом.
 */
export function isForbiddenToolName(name: string): boolean {
  const lower = name.toLowerCase();
  if (!lower.includes('ask_my_files')) return false;
  return /__ask_my_files$/.test(lower) || /__ask_my_files_[0-9a-f]{4,}$/.test(lower);
}

export const BASE_PROMPT = [
  'Ты — помощник hds в анонимной зоне проекта.',
  'Рабочая папка — текущий каталог; файлы вне неё недоступны.',
  'Реальные значения заменены метками вида [PERSON_1] или [ORG_2|род]: используй метки как есть,',
  'не пытайся угадать значения и не придумывай новых меток.',
  'Отвечай по-русски.',
].join('\n');

export interface StartInput {
  project: string;
  provider: { kind: string; base_url: string; model: string; api_key: string };
  history?: Json[];
  session_id?: string;
  max_iterations?: number;
  subagents?: boolean;
}

export function toolPolicies(): Record<string, { enabled: boolean; autoApprove?: boolean }> {
  return {
    read_files: { enabled: true, autoApprove: true },
    search_codebase: { enabled: true, autoApprove: true },
    editor: { enabled: true, autoApprove: true },
    run_commands: { enabled: false },
    fetch_web_content: { enabled: false },
  };
}

/** Правила агента из <HOME>\rules\*.md (TR-3.8): кладёт hds, без путей и имён. */
export async function readRules(home: string | undefined): Promise<string> {
  if (!home) return '';
  const dir = path.join(home, 'rules');
  try {
    const names = (await fsp.readdir(dir)).filter((n) => n.toLowerCase().endsWith('.md')).sort();
    const parts: string[] = [];
    for (const name of names) parts.push((await fsp.readFile(path.join(dir, name), 'utf8')).trim());
    return parts.filter(Boolean).join('\n\n');
  } catch {
    return '';
  }
}

/** Имена MCP-серверов из файла настроек (для отчёта в start). */
export async function mcpServers(settingsPath: string | undefined): Promise<string[]> {
  if (!settingsPath) return [];
  try {
    const raw = JSON.parse(await fsp.readFile(settingsPath, 'utf8'));
    return Object.keys(raw?.mcpServers ?? {}).sort();
  } catch {
    return [];
  }
}

export interface SessionDeps {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  sdk: any;
  channel: Channel;
  workspace: string;
  notify: (event: Json) => void;
  /** Последний список инструментов запроса к модели (для turn_end). */
  onTools: (names: string[]) => void;
}

/** Собрать вход ClineCore.start. */
export async function buildStart(input: StartInput, deps: SessionDeps): Promise<Json> {
  const { sdk, channel, workspace, notify, onTools } = deps;
  const subagents = Boolean(input.subagents);
  const rules = await readRules(process.env.HOME ?? process.env.USERPROFILE);
  const hooks = buildHooks({ workspace, channel, subagents, notify });
  const beforeModel = async (context: Json) => {
    try {
      const tools = Array.isArray(context?.request?.tools) ? context.request.tools : [];
      const kept = tools.filter((t: Json) => !FORBIDDEN_IN_REQUEST(String(t?.name ?? '')));
      onTools(kept.map((t: Json) => String(t?.name ?? '')));
      return kept.length === tools.length ? undefined : { tools: kept };
    } catch {
      return undefined;
    }
  };
  const extraTools = buildToolSpecs(channel).map((spec) =>
    sdk.createTool({
      name: spec.name,
      description: spec.description,
      inputSchema: spec.inputSchema,
      timeoutMs: spec.timeoutMs,
      execute: spec.execute,
    }),
  );
  const history = Array.isArray(input.history) ? input.history : [];
  return {
    config: {
      providerId: input.provider.kind,
      modelId: input.provider.model,
      apiKey: input.provider.api_key,
      baseUrl: input.provider.base_url,
      cwd: workspace,
      workspaceRoot: workspace,
      systemPrompt: rules ? `${BASE_PROMPT}\n\n${rules}` : BASE_PROMPT,
      enableTools: true,
      enableSpawnAgent: subagents,
      enableAgentTeams: false,
      maxIterations: input.max_iterations ?? 12,
      hooks: { ...hooks, beforeModel },
      extraTools,
      // Явные пути пусты: hds не подключает сторонние плагины. Плагины профиля
      // (<HOME>\.cline\plugins, <HOME>\.agents\plugins) отсекает политика
      // `plugins` ниже — одних пустых путей недостаточно (A3.1, тест изоляции).
      pluginPaths: [],
      agentPluginPaths: [],
      checkpoint: { enabled: false },
      ...(input.session_id ? { sessionId: input.session_id } : {}),
    },
    source: 'hds-agent',
    interactive: false,
    toolPolicies: toolPolicies(),
    // Плагины профиля (включая Agent Plugin из <HOME>\.agents\plugins, чьи MCP
    // инструменты иначе попадают в сессию) выключаются этой политикой (TR-3.2 п.4).
    plugins: { '*': { enabled: false } },
    ...(history.length ? { initialMessages: history } : {}),
  };
}
