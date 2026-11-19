// Заглушка MCP-сервера `tavily` для приёмки A3.6 (stdio, JSON-RPC 2.0 построчно).
//
// Публикует `tavily_search`, `tavily_extract` и `ask_my_files`:
// последний обязан быть вырезан воркером из запроса к модели даже с префиксом
// сервера (`tavily__ask_my_files`) — TR-4.4.
//
// `tavily_search` возвращает «известное» значение, которое фильтр `mcp_in`
// обязан заменить меткой; журнал вызовов (HDS_STUB_LOG) показывает, что именно
// получила внешняя сторона после `mcp_out`.
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';

const LOG_FROM_KEY = (() => {
  const key = process.env.TAVILY_API_KEY ?? '';
  const parts = key.split('|');
  return parts.length > 1 ? parts[1] : process.env.HDS_STUB_LOG;
})();
const LOG = LOG_FROM_KEY;

function logCall(name, args) {
  if (!LOG) return;
  try {
    appendFileSync(LOG, `${JSON.stringify({ name, args })}\n`);
  } catch {
    /* журнал — вспомогательный, падать из-за него нельзя */
  }
}

const tools = [
  {
    name: 'search',
    description: 'Search the web (stand stub): echoes the query and a known value.',
    inputSchema: {
      type: 'object',
      properties: { query: { type: 'string' } },
      required: ['query'],
    },
  },
  {
    name: 'extract',
    description: 'Extract a page (stand stub).',
    inputSchema: { type: 'object', properties: { url: { type: 'string' } } },
  },
  {
    name: 'ask_my_files',
    description: 'Must not reach the model (forbidden tool name).',
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
        serverInfo: { name: 'tavily-stand-stub', version: '0.0.1' },
      });
      break;
    case 'ping':
      reply({});
      break;
    case 'tools/list':
      reply({ tools });
      break;
    case 'tools/call': {
      const name = params?.name;
      const args = params?.arguments ?? {};
      logCall(name, args);
      if (name === 'search') {
        // Ответ содержит известное значение: mcp_in обязан вернуть метку.
        reply({
          content: [
            {
              type: 'text',
              text: `TAVILY-STUB: по запросу «${args.query ?? ''}» найден договор, подписант Иванов Иван.`,
            },
          ],
        });
      } else if (name === 'extract') {
        reply({ content: [{ type: 'text', text: `TAVILY-STUB-EXTRACT:${args.url ?? ''}` }] });
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
