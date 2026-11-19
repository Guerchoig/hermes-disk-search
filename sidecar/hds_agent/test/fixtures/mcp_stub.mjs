// Заглушка MCP (stdio, JSON-RPC 2.0, построчно) для e2e — по образцу
// tools/parity/agent_a0_sdk/mcp_echo_server.mjs. Публикует
// search_local_files (эхо), ask_my_files (должен быть вырезан воркером из
// запроса к модели — TR-4.4) и leak (возвращает «известное» значение, которое
// фильтр mcp_in обязан заменить меткой).
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';

// Журнал вызовов для тестов (HDS_STUB_LOG): что заглушка реально получила.
const LOG = process.env.HDS_STUB_LOG;

function logCall(name, args) {
  if (!LOG) return;
  try {
    appendFileSync(LOG, `${JSON.stringify({ name, args })}\n`);
  } catch {
    /* журнал — вспомогательный, падать из-за него нельзя */
  }
}

const TOOLS = [
  {
    name: 'search_local_files',
    description: 'Echo query back.',
    inputSchema: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] },
  },
  {
    name: 'ask_my_files',
    description: 'Must not reach the model.',
    inputSchema: { type: 'object', properties: { question: { type: 'string' } } },
  },
  {
    name: 'leak',
    description: 'Returns a known entity value.',
    inputSchema: { type: 'object', properties: {} },
  },
];

const send = (payload) => process.stdout.write(`${JSON.stringify(payload)}\n`);

createInterface({ input: process.stdin }).on('line', (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = msg;
  const reply = (result) => id !== undefined && send({ jsonrpc: '2.0', id, result });
  switch (method) {
    case 'initialize':
      reply({
        protocolVersion: params?.protocolVersion ?? '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'hds-e2e-stub', version: '0.0.1' },
      });
      break;
    case 'ping':
      reply({});
      break;
    case 'tools/list':
      reply({ tools: TOOLS });
      break;
    case 'tools/call': {
      const name = params?.name;
      const args = params?.arguments ?? {};
      logCall(name, args);
      if (name === 'search_local_files') {
        reply({ content: [{ type: 'text', text: `ECHO:${args.query ?? ''}` }] });
      } else if (name === 'leak') {
        reply({
          content: [{ type: 'text', text: 'Договор подписал Иванов Иван' }],
          // Сторонний сервер может вернуть значение не в `content`: mcp_in
          // обязан фильтровать весь результат (TR-6.3).
          structuredContent: { owner: 'Иванов Иван', note: 'ok' },
        });
      } else {
        reply({ content: [{ type: 'text', text: 'ASK-MY-FILES-SHOULD-NOT-RUN' }] });
      }
      break;
    }
    case 'resources/list':
      reply({ resources: [] });
      break;
    case 'prompts/list':
      reply({ prompts: [] });
      break;
    default:
      if (id !== undefined) send({ jsonrpc: '2.0', id, error: { code: -32601, message: 'not found' } });
  }
});
