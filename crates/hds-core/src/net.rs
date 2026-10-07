//! Сетевой периметр (M3 T1, `PLAN_M3_NETWORK_PERIMETER.md` §3.1): разбор
//! заголовков reverse-proxy.
//!
//! Приложение слушает `127.0.0.1`, наружу его публикует внешний прокси
//! (Caddy/nginx с TLS). Реальный IP клиента в этом случае есть только в
//! `X-Forwarded-For`, а схема (http/https) — в `X-Forwarded-Proto`. Оба
//! заголовка клиент может подделать, поэтому **по умолчанию они
//! игнорируются**: доверие включается ключом конфига `trusted_proxy: true`
//! (прокси в приватной сети, прямой доступ к `:8787`/`:8765` снаружи закрыт).

use crate::config::{dig, Config};

/// Доверять ли заголовкам `X-Forwarded-For`/`X-Forwarded-Proto` (`trusted_proxy`
/// в конфиге). Дефолт — `false`: сидящий напрямую клиент не должен уметь
/// подменить свой IP заголовком (аудит/rate-limit ключуются по real-IP).
pub fn trusted_proxy(cfg: &Config) -> bool {
    dig(cfg, "trusted_proxy")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Реальный клиентский IP: без доверенного прокси — `remote_addr`; при
/// `trusted_proxy` — **последняя (правая)** запись `X-Forwarded-For`: её
/// дописал наш прокси к уже полученному запросу, клиент не может её
/// подделать (свои записи он может дописать только слева). Пустые/мусорные
/// записи пропускаем; нет валидных — `remote_addr`.
pub fn real_client_ip(remote_addr: &str, xff: Option<&str>, trusted: bool) -> String {
    if trusted {
        if let Some(v) = xff {
            // Правая запись XFF — последняя непустая (идём с конца).
            if let Some(last) = v.split(',').rev().map(str::trim).find(|s| !s.is_empty()) {
                return last.to_string();
            }
        }
    }
    remote_addr.to_string()
}

/// Схема запроса (для аудита/журнала): при доверенном прокси — из
/// `X-Forwarded-Proto` (`https`, если прокси терминирует TLS), иначе `http`.
pub fn request_scheme(xfp: Option<&str>, trusted: bool) -> &'static str {
    if trusted {
        if let Some(v) = xfp {
            if v.eq_ignore_ascii_case("https") {
                return "https";
            }
        }
    }
    "http"
}

/// Лимит тела запроса в байтах (`http.max_body_kb`, дефолт 512 КиБ;
/// 0 — без лимита — не рекомендуется). Превышение → `413` (M3 T4, ревью F8).
pub fn max_body_bytes(cfg: &Config) -> usize {
    let kb = dig(cfg, "http.max_body_kb")
        .and_then(|v| v.as_u64())
        .unwrap_or(512);
    if kb == 0 {
        return usize::MAX; // 0 — без лимита (не рекомендуется)
    }
    usize::try_from(kb.saturating_mul(1024)).unwrap_or(usize::MAX)
}

/// Предел одновременных соединений (`http.max_conns`, дефолт 32; 0 — без
/// предела). Сверх предела — короткий `503` без спавна потоков (M3 T4).
pub fn max_conns(cfg: &Config) -> usize {
    dig(cfg, "http.max_conns")
        .and_then(|v| v.as_u64())
        .map(|v| usize::try_from(v).unwrap_or(0))
        .unwrap_or(32)
}

#[cfg(test)]
mod tests {
    use super::{real_client_ip, request_scheme, trusted_proxy};
    use serde_yaml::Value;

    #[test]
    fn trusted_proxy_defaults_false() {
        let cfg: Value = serde_yaml::from_str("index:\n  roots: []").unwrap();
        assert!(
            !trusted_proxy(&cfg),
            "без ключа заголовки прокси игнорируются"
        );
        let cfg: Value = serde_yaml::from_str("trusted_proxy: false").unwrap();
        assert!(!trusted_proxy(&cfg));
        let cfg: Value = serde_yaml::from_str("trusted_proxy: true").unwrap();
        assert!(trusted_proxy(&cfg));
        let cfg: Value = serde_yaml::from_str("trusted_proxy: garbage").unwrap();
        assert!(!trusted_proxy(&cfg), "не-bool — false");
    }

    /// Без `trusted_proxy` XFF игнорируется — подделка IP заголовком невозможна.
    #[test]
    fn real_ip_ignores_xff_without_trust() {
        assert_eq!(
            real_client_ip("127.0.0.1:1000", Some("1.2.3.4"), false),
            "127.0.0.1:1000"
        );
        assert_eq!(
            real_client_ip("127.0.0.1:1000", Some("1.2.3.4"), true),
            "1.2.3.4"
        );
        // Правая запись — от нашего прокси; клиентские подделки слева отбрасываются.
        assert_eq!(
            real_client_ip("127.0.0.1:1000", Some("9.9.9.9, 1.2.3.4"), true),
            "1.2.3.4"
        );
        // Пустые/пробельные записи пропускаем; нет валидных — remote_addr.
        assert_eq!(
            real_client_ip("127.0.0.1:1000", Some(" , "), true),
            "127.0.0.1:1000"
        );
        assert_eq!(
            real_client_ip("127.0.0.1:1000", None, true),
            "127.0.0.1:1000"
        );
    }

    #[test]
    fn scheme_from_proto_only_when_trusted() {
        assert_eq!(request_scheme(Some("https"), false), "http");
        assert_eq!(request_scheme(Some("HTTPS"), true), "https");
        assert_eq!(request_scheme(Some("http"), true), "http");
        assert_eq!(request_scheme(None, true), "http");
        assert_eq!(
            request_scheme(Some("ftp"), true),
            "http",
            "неизвестное — http"
        );
    }

    /// M3 T4: лимит тела (дефолт 512 КиБ; 0 — без лимита) и соединений (32).
    #[test]
    fn body_and_conn_limits() {
        use super::{max_body_bytes, max_conns};
        let d = serde_yaml::from_str::<serde_yaml::Value>("").unwrap();
        assert_eq!(max_body_bytes(&d), 512 * 1024, "дефолт 512 КиБ");
        assert_eq!(max_conns(&d), 32, "дефолт 32 соединения");
        let t =
            serde_yaml::from_str::<serde_yaml::Value>("http:\n  max_body_kb: 64\n  max_conns: 8")
                .unwrap();
        assert_eq!(max_body_bytes(&t), 64 * 1024);
        assert_eq!(max_conns(&t), 8);
        let off =
            serde_yaml::from_str::<serde_yaml::Value>("http:\n  max_body_kb: 0\n  max_conns: 0")
                .unwrap();
        assert_eq!(max_body_bytes(&off), usize::MAX, "0 — без лимита");
        assert_eq!(max_conns(&off), 0, "0 — без предела");
    }
}
