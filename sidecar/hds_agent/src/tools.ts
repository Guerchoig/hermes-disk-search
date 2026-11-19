// Собственные инструменты воркера (TR-3.5, П-4): office_read / office_edit /
// office_create и ask_user. Выполнение — только через host_call: файлы Office
// читает и правит Python-воркер hds_anonymize (через Rust), вопрос
// пользователю задаёт UI hds. Пути проверяет hds-guard (beforeTool) и
// повторно — Rust (crates/hds-agent/src/paths.rs).

import type { Channel, Json } from './rpc.ts';

export interface ToolSpec {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  execute: (input: Json) => Promise<Json>;
  timeoutMs?: number;
}

export const OFFICE_TOOLS = ['office_read', 'office_edit', 'office_create'] as const;
export const ASK_USER = 'ask_user';

const ASK_TIMEOUT_MS = 30 * 60_000;
const OFFICE_TIMEOUT_MS = 5 * 60_000;

function text(value: Json): string {
  return typeof value === 'string' ? value : JSON.stringify(value);
}

export function buildToolSpecs(channel: Channel): ToolSpec[] {
  return [
    {
      name: 'office_read',
      description:
        'Прочитать документ Office (docx, xlsx, pptx) в рабочей папке проекта. ' +
        'Возвращает текст: абзацы с номерами, таблицы в markdown, листы и ячейки ' +
        'с адресами, слайды. Для docx/xlsx/pptx используй только этот инструмент, ' +
        'а не read_files.',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Путь к файлу внутри рабочей папки.' },
        },
        required: ['path'],
      },
      timeoutMs: OFFICE_TIMEOUT_MS,
      execute: async (input) => {
        const result = await channel.host('office_read', { path: input?.path }, OFFICE_TIMEOUT_MS);
        return text(result?.markdown ?? result);
      },
    },
    {
      name: 'office_edit',
      description:
        'Изменить документ Office (docx, xlsx, pptx) операциями. Операции (поле op): ' +
        'replace_text {find, replace, regex?}; set_cell (docx: {table, row, col, text}; ' +
        'xlsx: {sheet?, cell, value}); add_row {table|sheet, values[]}; ' +
        'add_column {table, header?} или {sheet, values[]}; insert_paragraph {index, text, style?}; ' +
        'delete_paragraph {index}. Номера абзацев — из office_read. Форматирование, ' +
        'формулы и даты сохраняются.',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Путь к файлу внутри рабочей папки.' },
          ops: {
            type: 'array',
            description: 'Список операций по порядку.',
            items: { type: 'object' },
          },
        },
        required: ['path', 'ops'],
      },
      timeoutMs: OFFICE_TIMEOUT_MS,
      execute: async (input) => {
        const ops = Array.isArray(input?.ops) ? input.ops : [];
        const result = await channel.host(
          'office_edit',
          { path: input?.path, ops },
          OFFICE_TIMEOUT_MS,
        );
        return text({ applied: result?.applied ?? 0, kind: result?.kind ?? '' });
      },
    },
    {
      name: 'office_create',
      description:
        'Создать новый документ Office в рабочей папке. kind: docx (content.markdown), ' +
        'xlsx (content.sheets: [{name, rows: [[...]]}] или content.markdown с таблицами), ' +
        'pptx (content.slides: [{title, bullets[]}]). Существующий файл не перезаписывается.',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Путь нового файла внутри рабочей папки.' },
          kind: { type: 'string', enum: ['docx', 'xlsx', 'pptx'] },
          content: { type: 'object', description: 'Содержимое по виду документа.' },
        },
        required: ['path', 'kind', 'content'],
      },
      timeoutMs: OFFICE_TIMEOUT_MS,
      execute: async (input) => {
        const result = await channel.host(
          'office_create',
          { path: input?.path, kind: input?.kind, content: input?.content ?? {} },
          OFFICE_TIMEOUT_MS,
        );
        return text({ created: input?.path ?? '', kind: result?.kind ?? input?.kind ?? '' });
      },
    },
    {
      name: ASK_USER,
      description:
        'Задать вопрос пользователю и дождаться ответа. Используй, только если без ' +
        'ответа задачу выполнить нельзя.',
      inputSchema: {
        type: 'object',
        properties: { question: { type: 'string', description: 'Вопрос пользователю.' } },
        required: ['question'],
      },
      timeoutMs: ASK_TIMEOUT_MS,
      execute: async (input) => {
        const result = await channel.host(
          'ask_user',
          { question: String(input?.question ?? '') },
          ASK_TIMEOUT_MS,
        );
        return text(result?.answer ?? result);
      },
    },
  ];
}
