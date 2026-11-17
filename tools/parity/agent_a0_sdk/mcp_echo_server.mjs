// Минимальный MCP-сервер (stdio, JSON-RPC 2.0, построчный транспорт).
// Нужен спайкам A0, чтобы проверять MCP-инструменты без ключей и внешней сети:
// S3 (хуки beforeTool/afterTool на MCP), S4 (политики), S7 (подключение MCP).
//
// Отвечает на: initialize, tools/list, tools/call, ping, resources/list (пусто).
// Инструменты: echo (возвращает аргументы как текст), leak (возвращает строку,
// которую afterTool должен подменить), slow (спит 5 с — проверка таймаута хука).
import { createInterface } from 'node:readline';

const TOOLS = [
  {
    name: 'echo',
    description: 'Echo arguments back as text.',
    inputSchema: {
      type: 'object',
      properties: { text: { type: 'string' } },
      required: ['text'],
    },
  },
  {
    name: 'leak',
    description: 'Returns a marker string that a filter must replace.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'slow',
    description: 'Sleeps five seconds before answering.',
    inputSchema: { type: 'object', properties: {} },
  },
];

const send = (payload) => {
  process.stdout.write(`${JSON.stringify(payload)}\n`);
};

const rl = createInterface({ input: process.stdin });
rl.on('line', async (line) => {
  if (!line.trim()) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = msg;
  const reply = (result) => id !== undefined && send({ jsonrpc: '2.0', id, result });
  const fail = (code, message) => id !== undefined && send({ jsonrpc: '2.0', id, error: { code, message } });

  switch (method) {
    case 'initialize':
      reply({
        protocolVersion: params?.protocolVersion ?? '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'a0-echo-mcp', version: '0.0.1' },
      });
      break;
    case 'notifications/initialized':
    case 'notifications/cancelled':
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
      if (name === 'echo') {
        reply({ content: [{ type: 'text', text: `ECHO:${args.text ?? ''}` }] });
      } else if (name === 'leak') {
        reply({ content: [{ type: 'text', text: 'LEAK-MARKER-ORIGINAL' }] });
      } else if (name === 'slow') {
        setTimeout(() => reply({ content: [{ type: 'text', text: 'SLOW-DONE' }] }), 5000);
      } else {
        fail(-32602, `unknown tool: ${name}`);
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
      fail(-32601, `method not found: ${method}`);
  }
});
