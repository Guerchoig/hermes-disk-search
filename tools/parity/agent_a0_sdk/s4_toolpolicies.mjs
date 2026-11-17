// S4 — отключение инструментов через toolPolicies: они не должны попадать в
// список tools запроса к модели (а не просто требовать подтверждения).
import path from 'node:path';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult } from './stand.mjs';

const sandbox = applySandboxEnv(path.join(OUT_ROOT, 's4', 'sandbox'));
const cwd = sandbox.dirs.cwd;

const CASES = [
  { name: 's4/baseline', toolPolicies: undefined },
  {
    name: 's4/no-shell-no-web',
    toolPolicies: {
      run_commands: { enabled: false },
      fetch_web_content: { enabled: false },
    },
  },
  {
    name: 's4/no-read-no-editor',
    toolPolicies: {
      read_files: { enabled: false },
      editor: { enabled: false },
    },
  },
  {
    name: 's4/only-editor',
    toolPolicies: {
      '*': { enabled: false },
      editor: { enabled: true },
    },
  },
  {
    // можно ли «дополнительно включить» инструменты, которых нет в наборе
    // headless-режима (apply_patch, ask_question, submit_and_exit)
    name: 's4/try-enable-extra',
    toolPolicies: {
      apply_patch: { enabled: true },
      ask_question: { enabled: true },
      submit_and_exit: { enabled: true },
    },
  },
  {
    // plan-режим: ожидаемо без editor (read-only)
    name: 's4/mode-plan',
    toolPolicies: undefined,
    mode: 'plan',
  },
];

const rows = [];
for (const testCase of CASES) {
  const { row } = await runSession({
    name: testCase.name,
    script: [{ text: 'ok' }],
    cwd,
    prompt: 'просто ответь',
    enableTools: true,
    toolPolicies: testCase.toolPolicies,
    mode: testCase.mode,
  });
  const first = row.mockLog[0] ?? {};
  rows.push({
    name: testCase.name,
    toolPolicies: testCase.toolPolicies ?? null,
    toolsInRequest: first.toolNames ?? [],
    toolCount: first.hasTools ?? 0,
    end: row.end,
    errors: row.errors,
  });
  console.log(`${testCase.name}: ${(first.toolNames ?? []).join(', ')}`);
}

const summary = { cases: rows };
writeResult(path.join('s4', 'result.json'), summary);
console.log('---');
for (const row of rows) {
  console.log(`${row.name}: ${row.toolCount} tools -> ${row.toolsInRequest.join(', ')}`);
}
