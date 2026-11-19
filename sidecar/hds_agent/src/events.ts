// Преобразование событий SDK в компактные события протокола §4.3.
//
// Rust деанонимизирует события рекурсивно (все строки), поэтому сюда идут
// только поля, нужные UI: текст, рассуждения, вызовы и результаты
// инструментов, завершение хода. Потоковые `chunk` не пересылаются — текст
// приходит в agent_event content_start/content_end.

import { toJsonSafe, type Json } from './rpc.ts';

const MAX_OUTPUT_CHARS = 20_000;

function clip(value: Json): Json {
  const safe = toJsonSafe(value);
  if (typeof safe === 'string') {
    return safe.length > MAX_OUTPUT_CHARS ? `${safe.slice(0, MAX_OUTPUT_CHARS)}…` : safe;
  }
  const text = JSON.stringify(safe ?? null);
  if (text.length <= MAX_OUTPUT_CHARS) return safe;
  return `${text.slice(0, MAX_OUTPUT_CHARS)}…`;
}

/** agent_event → событие протокола или null (не пересылается). */
export function mapAgentEvent(event: Json): Json | null {
  const type = String(event?.type ?? '');
  const meta = {
    ...(event?.agentId ? { agent_id: event.agentId } : {}),
    ...(event?.parentAgentId ? { parent_agent_id: event.parentAgentId } : {}),
  };
  switch (type) {
    case 'content_start':
      if (event.contentType === 'text') return { kind: 'text_delta', text: event.text ?? '', ...meta };
      if (event.contentType === 'reasoning') {
        return { kind: 'reasoning_delta', text: event.reasoning ?? '', ...meta };
      }
      if (event.contentType === 'tool') {
        return {
          kind: 'tool_use',
          tool: event.toolName ?? '',
          call_id: event.toolCallId ?? '',
          input: clip(event.input ?? null),
          ...meta,
        };
      }
      return null;
    case 'content_update':
      return null;
    case 'content_end':
      if (event.contentType === 'text') return { kind: 'text', text: event.text ?? '', ...meta };
      if (event.contentType === 'reasoning') {
        return { kind: 'reasoning', text: event.reasoning ?? '', ...meta };
      }
      if (event.contentType === 'tool') {
        return {
          kind: 'tool_result',
          tool: event.toolName ?? '',
          call_id: event.toolCallId ?? '',
          output: clip(event.output ?? null),
          ...(event.error ? { error: String(event.error) } : {}),
          duration_ms: event.durationMs ?? 0,
          ...meta,
        };
      }
      return null;
    case 'iteration_start':
      return { kind: 'iteration', iteration: event.iteration ?? 0, ...meta };
    case 'usage':
      return {
        kind: 'usage',
        input_tokens: event.inputTokens ?? 0,
        output_tokens: event.outputTokens ?? 0,
        total_input_tokens: event.totalInputTokens ?? 0,
        total_output_tokens: event.totalOutputTokens ?? 0,
        ...(event.totalCost !== undefined ? { total_cost: event.totalCost } : {}),
        ...meta,
      };
    case 'notice':
      return { kind: 'notice', notice: event.noticeType ?? '', text: event.message ?? '', ...meta };
    case 'error':
      return {
        kind: 'error',
        text: String(event.error?.message ?? event.error ?? ''),
        recoverable: Boolean(event.recoverable),
        ...meta,
      };
    case 'done':
      return {
        kind: 'agent_done',
        reason: event.reason ?? '',
        text: event.text ?? '',
        iterations: event.iterations ?? 0,
        usage: toJsonSafe(event.usage ?? null),
        ...meta,
      };
    default:
      return null;
  }
}

/** Итог хода: «успех» с пустым текстом и без usage — это сбой (A0 §5.1). */
export function turnOutcome(done: Json | null, ended: Json | null, errorText?: string): Json {
  const reason = String(done?.reason ?? ended?.reason ?? 'unknown');
  const text = String(done?.text ?? '');
  const usage = done?.usage ?? null;
  const tokens = Number(usage?.inputTokens ?? 0) + Number(usage?.outputTokens ?? 0);
  let ok = reason === 'completed';
  let error: string | undefined;
  if (ok && !text.trim() && tokens === 0) {
    ok = false;
    error = 'empty_response: модель не вернула ни текста, ни usage';
  } else if (!ok) {
    // Причина сбоя — из события error (например, privacy_filter_unavailable),
    // иначе хотя бы reason из ended/собственный reason.
    error = errorText && errorText.trim() ? errorText : reason;
  }
  return {
    kind: 'turn_end',
    ok,
    reason,
    text,
    usage: toJsonSafe(usage),
    ...(error ? { error } : {}),
  };
}
