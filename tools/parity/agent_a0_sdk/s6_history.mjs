// S6 — формат истории сессии и «Очистить историю» (TR-6.6).
//
// 1) где и в каком виде SDK хранит историю (файлы каталога сессии, sessions.db);
// 2) можно ли переписать историю при остановленной сессии (замена строки);
// 3) возобновляется ли разговор так, чтобы следующий запрос к модели содержал
//    уже исправленный текст: (а) start({config.sessionId}) по существующему id,
//    (б) новая сессия с initialMessages из перезаписанного файла.
import fs from 'node:fs';
import path from 'node:path';
import { applySandboxEnv, runSession, OUT_ROOT, writeResult } from './stand.mjs';

const ROOT = path.join(OUT_ROOT, 's6');
const sandbox = applySandboxEnv(path.join(ROOT, 'sandbox'));
const cwd = sandbox.dirs.cwd;
process.chdir(cwd);

const MARKER = 'Иванов';
const LABEL = '[PERSON_1]';
const result = { files: {}, steps: [] };

// ---- 1. сессия с marker ------------------------------------------------
const run1 = await runSession({
  name: 's6/run1',
  script: [{ text: `${MARKER} Иван Иванович подтвердил срок этапа 2` }],
  cwd,
  prompt: `Привет, ${MARKER} Иван Иванович, напомни срок этапа 2`,
});
result.steps.push({ step: 'run1', requests: run1.row.mockRequests, end: run1.row.end });

const messagesPath = run1.row.messagesPath;
const manifestPath = run1.row.manifestPath;
const sessionDir = path.dirname(messagesPath);
result.files.sessionDir = sessionDir;
result.files.listing = fs.readdirSync(sessionDir);
result.files.messagesPath = messagesPath;
result.files.manifestPath = manifestPath;
result.files.messagesKeys = Object.keys(JSON.parse(fs.readFileSync(messagesPath, 'utf8')));

// есть ли маркер в sessions.db (SQLite рядом) — важно для TR-6.6
const dbPath = path.join(sandbox.dirs.home, '.cline', 'data', 'db', 'sessions.db');
const dbHasMarker = fs.existsSync(dbPath)
  ? fs.readFileSync(dbPath).includes(Buffer.from(MARKER, 'utf8')) ||
    fs.readFileSync(dbPath).includes(Buffer.from(MARKER, 'utf16le'))
  : null;
result.files.sessionsDbHasMarker = dbHasMarker;
result.files.sessionsDbSize = fs.existsSync(dbPath) ? fs.statSync(dbPath).size : null;

// ---- 2. перезапись истории при остановленной сессии -------------------
function rewriteStrings(file) {
  const before = fs.readFileSync(file, 'utf8');
  const after = before.split(MARKER).join(LABEL);
  const tmp = `${file}.tmp`;
  fs.writeFileSync(tmp, after, 'utf8');
  fs.renameSync(tmp, file);
  return { changed: before !== after, occurrences: before.split(MARKER).length - 1 };
}

const rewriteMessages = rewriteStrings(messagesPath);
const rewriteManifest = rewriteStrings(manifestPath);
result.rewrite = { messages: rewriteMessages, manifest: rewriteManifest };

const storedAfter = JSON.parse(fs.readFileSync(messagesPath, 'utf8'));
result.storedMessagesAfterRewrite = storedAfter.messages.map((m) => ({
  role: m.role,
  text: (m.content ?? [])
    .map((c) => c.text ?? '')
    .join(' ')
    .slice(0, 200),
}));
result.markerLeftInFile = fs.readFileSync(messagesPath, 'utf8').includes(MARKER);

// ---- 3а. возобновление по существующему sessionId ---------------------
const oldId = run1.row.sessionId;
const resumeById = await runSession({
  name: 's6/resume-by-id',
  script: [{ text: 'продолжаю' }],
  cwd,
  prompt: 'а что по этапу 3?',
  sessionIdForConfig: oldId,
});
result.steps.push({
  step: 'resume-by-sessionId',
  sessionIdRequested: oldId,
  sessionIdActual: resumeById.row.sessionId,
  sameId: resumeById.row.sessionId === oldId,
  requests: resumeById.row.mockRequests,
  messageCountSeenByModel: resumeById.row.mockLog.map((l) => l.messageCount),
  errors: resumeById.row.errors,
  end: resumeById.row.end,
});

// ---- 3б. новая сессия с initialMessages из файла ----------------------
const initialMessages = storedAfter.messages.map((m) => ({
  role: m.role,
  content: m.content,
}));
const resumeWithSeed = await runSession({
  name: 's6/resume-with-initial',
  script: [{ text: 'продолжаю' }],
  cwd,
  prompt: 'а что по этапу 3?',
  initialMessages,
  enableTools: false,
});
const seeded = resumeWithSeed.row.mockLog[0] ?? {};
result.steps.push({
  step: 'resume-with-initialMessages',
  requests: resumeWithSeed.row.mockRequests,
  messageCountSeenByModel: resumeWithSeed.row.mockLog.map((l) => l.messageCount),
  firstRequestHasLabel: JSON.stringify(seeded.bodyFull ?? '').includes(LABEL),
  firstRequestHasMarker: JSON.stringify(seeded.bodyFull ?? '').includes(MARKER),
  errors: resumeWithSeed.row.errors,
  end: resumeWithSeed.row.end,
});

// ---- 3в. рекомендуемый вариант для «Очистить историю»: тот же sessionId
//          + initialMessages из перезаписанного файла ----------------------
const resumeSameIdSeeded = await runSession({
  name: 's6/resume-same-id-seeded',
  script: [{ text: 'продолжаю' }],
  cwd,
  prompt: 'а что по этапу 3?',
  sessionIdForConfig: oldId,
  initialMessages,
  enableTools: false,
});
const seededSame = resumeSameIdSeeded.row.mockLog[0] ?? {};
result.steps.push({
  step: 'resume-same-id-with-initialMessages',
  sessionIdRequested: oldId,
  sessionIdActual: resumeSameIdSeeded.row.sessionId,
  sameId: resumeSameIdSeeded.row.sessionId === oldId,
  requests: resumeSameIdSeeded.row.mockRequests,
  messageCountSeenByModel: resumeSameIdSeeded.row.mockLog.map((l) => l.messageCount),
  firstRequestHasLabel: JSON.stringify(seededSame.bodyFull ?? '').includes(LABEL),
  firstRequestHasMarker: JSON.stringify(seededSame.bodyFull ?? '').includes(MARKER),
  errors: resumeSameIdSeeded.row.errors,
  end: resumeSameIdSeeded.row.end,
});

writeResult(path.join('s6', 'result.json'), result);
console.log(JSON.stringify({
  messagesKeys: result.files.messagesKeys,
  sessionsDbHasMarker: dbHasMarker,
  rewrite: result.rewrite,
  resumeById: result.steps[1],
  resumeSeeded: result.steps[2],
}, null, 2));
