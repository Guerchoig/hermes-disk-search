// S5 — суб-агенты: наследуют ли они наш fetch, хуки, cwd и видны ли их события.
import fs from 'node:fs';
import path from 'node:path';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult } from './stand.mjs';

const ROOT = path.join(OUT_ROOT, 's5');
const sandbox = applySandboxEnv(path.join(ROOT, 'sandbox'));
const cwd = sandbox.dirs.cwd;
process.chdir(cwd);
fs.writeFileSync(path.join(cwd, 'allowed.txt'), 'ALLOWED-CONTENT-42\n', 'utf8');

const hookCalls = [];
const guard = {
  beforeTool: (context) => {
    hookCalls.push({ tool: context.tool?.name, input: JSON.stringify(context.input ?? {}).slice(0, 200) });
    if (context.tool?.name === 'run_commands') {
      return { skip: true, reason: 'hds-guard: терминал выключен' };
    }
    return undefined;
  },
};

const run = await runSession({
  name: 's5/run',
  script: [
    { toolCalls: [{ name: 'spawn_agent', arguments: { systemPrompt: 'ты субагент', task: 'прочитай allowed.txt' } }] },
    { toolCalls: [{ name: 'read_files', arguments: { files: [{ path: 'allowed.txt' }] } }] },
    { text: 'субагент: прочитал' },
    { text: 'родитель: готово' },
  ],
  cwd,
  enableTools: true,
  enableSpawnAgent: true,
  hooks: guard,
  prompt: 'запусти субагента',
  maxIterations: 6,
  timeoutMs: 120_000,
});

const summary = {
  requests: run.row.mockRequests,
  egressCalls: run.row.egressCalls,
  spawnToolPresent: (run.row.mockLog[0]?.toolNames ?? []).includes('spawn_agent'),
  toolNamesByRequest: run.row.mockLog.map((l) => (l.toolNames ?? []).length),
  hookCalls,
  // у субагента другой system prompt — видно по телу запроса
  systemPrompts: run.row.mockLog.map((l) => {
    const sys = (l.bodyFull?.messages ?? []).find((m) => m.role === 'system');
    const text = typeof sys?.content === 'string' ? sys.content : JSON.stringify(sys?.content ?? '');
    return text.slice(0, 80);
  }),
  toolResults: run.row.mockLog.slice(1).flatMap((l) => l.toolResultSnippets ?? []).map((t) => t.slice(0, 200)),
  eventTypes: run.row.types,
  end: run.row.end,
  errors: run.row.errors,
  equalFetch: run.row.egressCalls === run.row.mockRequests,
};

writeResult(path.join('s5', 'result.json'), summary);
console.log(JSON.stringify(summary, null, 2).slice(0, 2500));
