// Матрица путей TR-3.10 (а) и политика форматов (TR-3.5) — без SDK и сети.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { after, before, test } from 'node:test';

import { checkWithin, lexicalReject, withTimeout } from '../src/guard.ts';
import { collectPaths, formatReject, guardDecision } from '../src/hooks.ts';

let root;
let workspace;
let outside;

before(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'hds-guard-'));
  workspace = path.join(root, 'workspace');
  outside = path.join(root, 'outside');
  fs.mkdirSync(path.join(workspace, 'sub'), { recursive: true });
  fs.mkdirSync(outside, { recursive: true });
  fs.writeFileSync(path.join(workspace, 'allowed.txt'), 'ok');
  fs.writeFileSync(path.join(outside, 'secret.txt'), 'secret');
  fs.symlinkSync(outside, path.join(workspace, 'link-out'), 'junction');
});

after(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('dangerous paths are rejected', async () => {
  const matrix = [
    ['parent traversal', '..\\outside\\secret.txt'],
    ['absolute outside', () => path.join(outside, 'secret.txt')],
    ['junction outside', 'link-out\\secret.txt'],
    ['UNC', '\\\\server\\share\\file.txt'],
    ['UNC slashes', '//server/share/file.txt'],
    ['device namespace', '\\\\?\\C:\\Windows\\win.ini'],
    ['device dot', '\\\\.\\PhysicalDrive0'],
    ['ads stream', 'allowed.txt:hidden'],
    ['device name', 'NUL'],
    ['device with ext', 'sub\\com1.txt'],
    ['empty', '   '],
    ['not a string', 42],
  ];
  for (const [name, value] of matrix) {
    const candidate = typeof value === 'function' ? value() : value;
    const check = await checkWithin(workspace, candidate);
    assert.equal(check.ok, false, `${name} должен быть отклонён`);
    assert.ok(check.reason.length > 0, `${name}: причина`);
  }
});

test('paths inside workspace are allowed, including missing ones', async () => {
  for (const candidate of [
    'allowed.txt',
    path.join(workspace, 'allowed.txt'),
    'sub\\..\\allowed.txt',
    'sub\\new-file.md',
    'deep\\missing\\dir\\x.txt',
  ]) {
    const check = await checkWithin(workspace, candidate);
    assert.equal(check.ok, true, `${candidate}: ${check.reason ?? ''}`);
  }
});

test('lexical rules never throw', () => {
  for (const v of [undefined, null, {}, [], '\0', 'C:\\a\\b', 'C:relative']) {
    assert.doesNotThrow(() => lexicalReject(v));
  }
  assert.equal(lexicalReject('C:\\work\\a.txt'), null);
});

test('office formats are routed to office tools', () => {
  assert.match(formatReject('read_files', 'C:\\w\\a.docx'), /office_read/);
  assert.match(formatReject('editor', 'C:\\w\\a.xlsx'), /office_edit/);
  assert.match(formatReject('editor', 'C:\\w\\scan.pdf.md'), /только для чтения/);
  assert.match(formatReject('editor', 'C:\\w\\deck.pptx.md'), /только для чтения/);
  assert.equal(formatReject('read_files', 'C:\\w\\scan.pdf.md'), null);
  assert.equal(formatReject('editor', 'C:\\w\\notes.md'), null);
});

test('guard decisions per tool', async () => {
  const opts = { workspace, subagents: false };
  assert.equal(await guardDecision('read_files', { files: [{ path: 'allowed.txt' }] }, opts), undefined);
  assert.ok(await guardDecision('read_files', { files: [{ path: '..\\outside\\secret.txt' }] }, opts));
  assert.ok(await guardDecision('read_files', { files: [{ path: 'report.docx' }] }, opts));
  assert.ok(await guardDecision('editor', { path: 'report.xlsx', new_text: 'x' }, opts));
  assert.equal(await guardDecision('editor', { path: 'notes.md', new_text: 'x' }, opts), undefined);
  assert.ok(await guardDecision('run_commands', { commands: ['dir'] }, opts));
  assert.ok(await guardDecision('fetch_web_content', { requests: [] }, opts));
  assert.ok(await guardDecision('spawn_agent', {}, opts), 'суб-агенты выключены');
  assert.equal(await guardDecision('spawn_agent', {}, { workspace, subagents: true }), undefined);
  assert.equal(await guardDecision('search_codebase', { queries: ['x'] }, opts), undefined);
  assert.equal(await guardDecision('office_read', { path: 'a.docx' }, opts), undefined);
  assert.ok(await guardDecision('office_read', { path: 'link-out\\a.docx' }, opts));
  assert.ok(await guardDecision('office_read', {}, opts), 'без пути — отказ');
  assert.equal(await guardDecision('disk-search__search_local_files', { query: 'x' }, opts), undefined);
});

test('collectPaths finds nested and aliased path keys', () => {
  const paths = collectPaths({ files: [{ path: 'a' }, { file_path: 'b' }], cwd: 'c', other: { filePath: 'd' } });
  assert.deepEqual(paths.sort(), ['a', 'b', 'c', 'd']);
});

test('withTimeout falls back instead of hanging', async () => {
  const slow = new Promise((resolve) => setTimeout(() => resolve('late'), 500));
  assert.equal(await withTimeout(slow, 20, () => 'fallback'), 'fallback');
  assert.equal(await withTimeout(Promise.reject(new Error('x')), 100, () => 'safe'), 'safe');
});
