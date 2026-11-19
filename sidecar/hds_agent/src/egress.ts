// egressFetch — единственный сетевой выход SDK (TR-6.1, TR-6.5).
//
// Каждый запрос провайдеру (включая повторы и суб-агентов — A0 §3.2/§3.5) до
// отправки проходит `host_call egress_check {kind: "model", body}`: Rust
// заменяет значения метками по словарю vault и блокирует запрос с секретом.
// Любой отказ (нет снимка, фильтр упал, таймаут, канал закрыт) — fail-closed:
// запрос не уходит, SDK получает ошибку `privacy_filter_unavailable`.

import type { Channel, Json } from './rpc.ts';

export const FILTER_UNAVAILABLE = 'privacy_filter_unavailable';

type FetchInput = string | URL | Request;

function urlOf(input: FetchInput): string {
  if (typeof input === 'string') return input;
  if (input instanceof URL) return input.toString();
  return input.url;
}

async function bodyText(input: FetchInput, init?: RequestInit): Promise<string | null> {
  const body = init?.body ?? (input instanceof Request ? await input.clone().text() : null);
  if (body === null || body === undefined) return null;
  if (typeof body === 'string') return body;
  if (body instanceof URLSearchParams) return body.toString();
  if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) {
    return Buffer.from(body as ArrayBuffer).toString('utf8');
  }
  if (typeof Blob !== 'undefined' && body instanceof Blob) return body.text();
  // ReadableStream, FormData и прочее разобрать надёжно нельзя — не отправляем.
  throw new Error(`${FILTER_UNAVAILABLE}: тело запроса неизвестного типа`);
}

export interface EgressStats {
  calls: number;
  blocked: number;
}

/** Собрать fetch для ClineCore.create({ fetch }). */
export function makeEgressFetch(channel: Channel, base: typeof fetch = fetch) {
  const stats: EgressStats = { calls: 0, blocked: 0 };
  const egressFetch = async (input: FetchInput, init?: RequestInit): Promise<Response> => {
    stats.calls += 1;
    const url = urlOf(input);
    let checked: Json;
    let text: string | null;
    try {
      text = await bodyText(input, init);
      let parsed: Json = null;
      if (text !== null) {
        try {
          parsed = JSON.parse(text);
        } catch {
          parsed = text;
        }
      }
      checked = await channel.host('egress_check', { kind: 'model', body: parsed });
    } catch (error) {
      stats.blocked += 1;
      const reason = String((error as Error)?.message ?? error);
      throw new Error(
        reason.startsWith(FILTER_UNAVAILABLE) ? reason : `${FILTER_UNAVAILABLE}: ${reason}`,
      );
    }
    const filtered = checked?.body;
    const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
    headers.delete('content-length');
    const method = init?.method ?? (input instanceof Request ? input.method : undefined);
    const next: RequestInit = { ...init, headers, ...(method ? { method } : {}) };
    if (text !== null) {
      next.body = typeof filtered === 'string' ? filtered : JSON.stringify(filtered ?? null);
    }
    if (input instanceof Request && !init?.signal) next.signal = input.signal;
    return base(url, next);
  };
  return Object.assign(egressFetch, { stats });
}
