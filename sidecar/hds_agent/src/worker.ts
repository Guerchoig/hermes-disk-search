// hds_agent worker — точка входа (dist/worker.mjs).
//
// Запускается hds (crates/hds-agent/src/worker.rs) с очищенным окружением и
// cwd = <P>\workspace. Протокол — NDJSON §4.3 (src/rpc.ts):
//   hello → ready; start → done {session_id, tools}; send → done {accepted}
//   + события, ход завершается событием {kind: "turn_end"}; abort, stop,
//   shutdown → done.

import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { mapAgentEvent, turnOutcome } from './events.ts';
import { makeEgressFetch } from './egress.ts';
import { Channel, redirectConsole, type DownMessage, type Json } from './rpc.ts';
import { buildStart, mcpServers, type StartInput } from './session.ts';

redirectConsole();

const channel = new Channel();
const workspace = process.cwd();
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let sdk: any = null;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let cline: any = null;
let sessionId: string | null = null;
let unsubscribe: (() => void) | null = null;
let busy = false;
let currentId = 'event';
let lastDone: Json = null;
let lastTools: string[] = [];

function sdkVersion(): string {
  try {
    const here = path.dirname(fileURLToPath(import.meta.url));
    const pkg = path.join(here, '..', 'node_modules', '@cline', 'sdk', 'package.json');
    return existsSync(pkg) ? String(JSON.parse(readFileSync(pkg, 'utf8')).version ?? '') : '';
  } catch {
    return '';
  }
}

function errorText(error: unknown): string {
  return String((error as Error)?.message ?? error);
}

const notify = (event: Json) => channel.event(currentId, event);

async function ensureCore(): Promise<void> {
  if (cline) return;
  sdk = await import('@cline/sdk');
  cline = await sdk.ClineCore.create({
    clientName: 'hds-agent',
    backendMode: 'local',
    fetch: makeEgressFetch(channel),
  });
}

function onSessionEvent(event: Json): void {
  try {
    if (event?.type === 'agent_event') {
      const inner = event.payload?.event;
      if (inner?.type === 'done' && !inner.parentAgentId) lastDone = inner;
      const mapped = mapAgentEvent(inner);
      if (mapped) notify(mapped);
    } else if (event?.type === 'ended') {
      notify({ kind: 'ended', reason: event.payload?.reason ?? '' });
    } else if (event?.type === 'status') {
      notify({ kind: 'status', status: event.payload?.status ?? '' });
    }
  } catch (error) {
    channel.error(`event: ${errorText(error)}`);
  }
}

async function closeSession(): Promise<void> {
  if (unsubscribe) {
    try {
      unsubscribe();
    } catch {
      /* уже отписан */
    }
    unsubscribe = null;
  }
  if (cline && sessionId) {
    try {
      await cline.stop(sessionId);
    } catch {
      /* сессия уже остановлена */
    }
  }
  sessionId = null;
  busy = false;
}

async function start(message: DownMessage): Promise<void> {
  await ensureCore();
  await closeSession();
  const input = message as unknown as StartInput;
  const startInput = await buildStart(input, {
    sdk,
    channel,
    workspace,
    notify,
    onTools: (names) => {
      lastTools = names;
    },
  });
  const started = await cline.start(startInput);
  sessionId = String(started.sessionId);
  unsubscribe = cline.subscribe(onSessionEvent, { sessionId });
  const servers = await mcpServers(process.env.CLINE_MCP_SETTINGS_PATH);
  channel.done(message.id, true, {
    session_id: sessionId,
    tools: ['read_files', 'search_codebase', 'editor', 'office_read', 'office_edit', 'office_create', 'ask_user'],
    mcp_servers: servers,
    plugin_issues: (started.pluginIssues ?? []).map((p: Json) => String(p?.name ?? '')),
  });
}

function send(message: DownMessage): void {
  if (!cline || !sessionId) {
    channel.done(message.id, false, {}, 'session_not_started');
    return;
  }
  if (busy) {
    channel.done(message.id, false, {}, 'turn_in_progress');
    return;
  }
  busy = true;
  currentId = String(message.id ?? 'event');
  lastDone = null;
  const id = sessionId;
  channel.done(message.id, true, { accepted: true });
  cline
    .send({ sessionId: id, prompt: String(message.text ?? '') })
    .then(
      () => notify({ ...turnOutcome(lastDone, null), tools: lastTools }),
      (error: unknown) =>
        notify({ kind: 'turn_end', ok: false, reason: 'error', text: '', error: errorText(error), tools: lastTools }),
    )
    .finally(() => {
      busy = false;
    });
}

async function handle(message: DownMessage): Promise<void> {
  switch (message.type) {
    case 'hello':
      channel.ready(message.id, { node: process.version, sdk: sdkVersion(), tools: [] });
      return;
    case 'start':
      return start(message);
    case 'send':
      return send(message);
    case 'abort':
      if (cline && sessionId) await cline.abort(sessionId, 'user_abort');
      channel.done(message.id, true, {});
      return;
    case 'stop':
      await closeSession();
      channel.done(message.id, true, {});
      return;
    case 'shutdown':
      await shutdown(message.id);
      return;
    default:
      channel.done(message.id, false, {}, `unknown_command: ${message.type}`);
  }
}

async function shutdown(id?: string): Promise<void> {
  await closeSession();
  if (cline) {
    try {
      await cline.dispose('shutdown');
    } catch {
      /* best-effort */
    }
  }
  channel.done(id, true, {});
  channel.close();
  setTimeout(() => process.exit(0), 50);
}

process.on('unhandledRejection', (reason) => channel.error(`unhandledRejection: ${errorText(reason)}`));
process.on('uncaughtException', (error) => channel.error(`uncaughtException: ${errorText(error)}`, error?.stack));

channel.listen(
  (message) => {
    handle(message).catch((error) => channel.done(message.id, false, {}, errorText(error)));
  },
  () => {
    void shutdown();
  },
);
