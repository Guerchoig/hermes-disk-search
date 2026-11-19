// TR-3.10 г, е: ничего из профиля (плагины, MCP по «умолчанию») в сессию не попадает.
//
// 1. Плагин, положенный в <HOME>\.cline\plugins, не загружен: его инструмента нет
//    в tools. Формат плагина проверяется положительным контролем — тот же файл,
//    загруженный явно, даёт расширение (иначе тест был бы пустым).
// 2. MCP-сервер в файле настроек по умолчанию (путь SDK без CLINE_MCP_SETTINGS_PATH)
//    не загружен: инструментов нет; грузится только файл, указанный hds.
// 3. Agent-plugin (plugin.json + mcp.json в <HOME>\.agents\plugins) не загружен.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

import { loadAgentPluginPackages, loadAgentPluginsFromPaths } from '@cline/core';
import { discoverPluginModulePaths } from '@cline/shared/storage';

import {
  MCP_STUB,
  WORKER,
  defaultHost,
  makeProject,
  nodeCommand,
  provider,
  startWorker,
  writeStubMcp,
} from './fixtures/harness.mjs';
import { startMockProvider, toolNames } from './fixtures/mock_provider.mjs';

const PLUGIN_SOURCE = `export default {
  name: 'hds-profile-plugin',
  manifest: { capabilities: ['tools'] },
  setup(api) {
    api.registerTool({
      name: 'profile_plugin_tool',
      description: 'Плагин из профиля загружаться не должен.',
      inputSchema: { type: 'object', properties: {} },
      execute: async () => 'PROFILE-PLUGIN-RAN',
    });
  },
};
`;

test('профиль: плагин и MCP по умолчанию не загружены', { timeout: 180_000 }, async (t) => {
  assert.ok(fs.existsSync(WORKER), 'нужна сборка: npm run build');
  const dirs = makeProject();
  writeStubMcp(dirs);

  const pluginDir = path.join(dirs.config, '.cline', 'plugins', 'hds-profile');
  fs.mkdirSync(pluginDir, { recursive: true });
  // SDK ищет модули плагинов только с расширениями .js/.ts (PLUGIN_FILE_EXTENSIONS).
  const pluginFile = path.join(pluginDir, 'plugin.ts');
  fs.writeFileSync(pluginFile, PLUGIN_SOURCE);

  const agentPluginDir = path.join(dirs.config, '.agents', 'plugins', 'hds-profile');
  fs.mkdirSync(agentPluginDir, { recursive: true });
  fs.writeFileSync(
    path.join(agentPluginDir, 'plugin.json'),
    JSON.stringify({ $schema: 'https://agent-plugins.org/schemas/1.0.0/plugin.schema.json', name: 'hds-agent-plugin', version: '0.0.1' }),
  );
  fs.writeFileSync(
    path.join(agentPluginDir, 'mcp.json'),
    JSON.stringify({
      $schema: 'https://agent-plugins.org/schemas/1.0.0/mcp.schema.json',
      mcpServers: {
        agentdecoy: { type: 'stdio', command: nodeCommand(), args: [MCP_STUB] },
      },
    }),
  );

  // Файл, который SDK взял бы по умолчанию (CLINE_DATA_DIR\settings\…), если бы
  // hds не задавал CLINE_MCP_SETTINGS_PATH: его серверы грузиться не должны.
  const decoySettings = path.join(dirs.state, '.cline', 'data', 'settings', 'cline_mcp_settings.json');
  fs.mkdirSync(path.dirname(decoySettings), { recursive: true });
  fs.writeFileSync(
    decoySettings,
    JSON.stringify({
      mcpServers: {
        decoy: { transport: { type: 'stdio', command: nodeCommand(), args: [MCP_STUB] }, timeoutSeconds: 30 },
      },
    }),
  );

  // Положительный контроль: фикстуры — настоящие плагины (иначе отсутствие их
  // инструментов в сессии ничего не доказывает).
  const loaded = await loadAgentPluginsFromPaths([pluginFile], { cwd: dirs.workspace });
  assert.deepEqual(loaded.map((p) => p.name), ['hds-profile-plugin'], 'Cline-плагин валиден');
  const discovered = discoverPluginModulePaths(path.join(dirs.config, '.cline', 'plugins'));
  assert.ok(
    discovered.some((p) => p.endsWith('plugin.ts')),
    `плагин лежит там, где SDK его ищет: ${JSON.stringify(discovered)}`,
  );
  const packages = await loadAgentPluginPackages({
    pluginPaths: [agentPluginDir],
    pluginDataRoot: path.join(dirs.root, 'plugin-data'),
  });
  assert.deepEqual(packages.diagnostics, [], 'Agent Plugin валиден');
  assert.deepEqual(packages.mcpServers.map((s) => s.serverName), ['agentdecoy'], 'в нём есть MCP-сервер');

  const mock = startMockProvider([{ text: 'готово' }]);
  const base = await mock.ready;
  const log = [];
  const worker = startWorker(dirs, defaultHost(log));
  t.after(async () => {
    await worker.stop();
    await mock.stop();
    fs.rmSync(dirs.root, { recursive: true, force: true });
  });

  await worker.call('hello', { protocol: 1 });
  const started = await worker.call(
    'start',
    { project: 'P0001', provider: provider(base), max_iterations: 4 },
    120_000,
  );
  assert.equal(started.ok, true, started.error);
  // Плагин профиля найден (значит, каталог просматривается), но отклонён
  // политикой: он попадает в plugin_issues, а его инструмент — нет в tools.
  assert.ok(
    started.result.plugin_issues.includes('hds-profile-plugin'),
    `плагин профиля обнаружен и отклонён: ${JSON.stringify(started.result.plugin_issues)}`,
  );
  await worker.call('send', { text: 'привет' });
  const end = await worker.waitEvent((e) => e.kind === 'turn_end', 120_000);
  assert.equal(end.ok, true, end.error);

  const names = toolNames(mock.requests[0]);
  for (const expected of ['stub__search_local_files', 'stub__leak']) {
    assert.ok(names.includes(expected), `${expected} из файла hds должен быть: ${names}`);
  }
  for (const forbidden of [
    'profile_plugin_tool',
    'decoy__leak',
    'agentdecoy__leak',
    'hds-agent-plugin.agentdecoy__leak',
  ]) {
    assert.ok(!names.includes(forbidden), `${forbidden} не должен попасть в tools: ${names}`);
    assert.ok(!end.tools.includes(forbidden), `${forbidden} не должен попасть в turn_end.tools`);
  }
  for (const name of names) {
    assert.ok(!name.toLowerCase().includes('ask_my_files'), `ask_my_files не должен попасть: ${name}`);
  }
});
