//! Реестр живых процессов агента — жизненный цикл проекта (TR-3.9, В-16).
//!
//! Один проект = один процесс Node-воркера. Процесс живёт, пока открыто не
//! больше `agent.max_live_projects` проектов (по умолчанию 2) и пока проект не
//! простаивал дольше `agent.idle_unload_min` (30 мин). Жертва при переполнении —
//! проект, который дольше всех не использовался (LRU по `last_used`).
//!
//! Время приходит параметром (`now`): фоновых потоков и «спящих» проверок нет.
//! Вкладка A5/служба зовёт [`AgentRegistry::reap_idle`] тиком с `Instant::now()`,
//! тесты — с любым нужным значением.
//!
//! Выгрузка = `stop` (`cline.stop`, мягкая остановка) + `shutdown`
//! (`Worker::shutdown` ждёт выхода ≤ 5 с, затем kill) и удаление записи. Реестр
//! отдаёт `&mut Gateway` на проект: одна активная сессия на проект, параллельные
//! ходы — домен UI (TR-3.9).

use std::time::{Duration, Instant};

use crate::gateway::{Gateway, GatewayError};
use crate::settings::AgentSettings;

/// Значение В-16: одновременно живых процессов проекта.
pub const DEFAULT_MAX_LIVE: usize = 2;
/// Значение В-16: простой, после которого проект выгружается (30 мин).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(30 * 60);

/// Живой процесс проекта.
struct Live {
    project: String,
    gateway: Gateway,
    last_used: Instant,
}

/// Реестр живых процессов (ключ — `P0001`).
pub struct AgentRegistry {
    max_live: usize,
    idle: Duration,
    live: Vec<Live>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::defaults()
    }
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("max_live", &self.max_live)
            .field("idle", &self.idle)
            .field("live", &self.projects())
            .finish()
    }
}

impl AgentRegistry {
    /// Реестр с явными границами. `max_live == 0` трактуется как 1: держать
    /// ни одного процесса и всё равно возвращать шлюз нельзя.
    pub fn new(max_live: usize, idle: Duration) -> Self {
        AgentRegistry {
            max_live: max_live.max(1),
            idle,
            live: Vec::new(),
        }
    }

    /// Границы из конфига: `agent.max_live_projects`, `agent.idle_unload_min`.
    pub fn from_settings(settings: &AgentSettings) -> Self {
        AgentRegistry::new(
            settings.max_live_projects,
            Duration::from_secs(settings.idle_secs),
        )
    }

    /// Значения по умолчанию (В-16: 2 проекта, 30 минут).
    pub fn defaults() -> Self {
        AgentRegistry::from_settings(&AgentSettings::default())
    }

    /// Сколько процессов разрешено держать.
    pub fn max_live(&self) -> usize {
        self.max_live
    }

    /// Простой, после которого проект выгружается.
    pub fn idle(&self) -> Duration {
        self.idle
    }

    /// Сколько проектов открыто.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Реестр пуст (ни одного процесса).
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Живые проекты и метки последнего использования (в порядке открытия).
    pub fn live(&self) -> Vec<(String, Instant)> {
        self.live
            .iter()
            .map(|entry| (entry.project.clone(), entry.last_used))
            .collect()
    }

    /// Идентификаторы живых проектов (в порядке открытия).
    pub fn projects(&self) -> Vec<String> {
        self.live
            .iter()
            .map(|entry| entry.project.clone())
            .collect()
    }

    /// Живой шлюз проекта; `None` — проект не открыт.
    pub fn get_mut(&mut self, project: &str) -> Option<&mut Gateway> {
        self.index(project).map(|i| &mut self.live[i].gateway)
    }

    /// Отметить использование проекта: `true`, если проект был открыт.
    pub fn touch(&mut self, project: &str, now: Instant) -> bool {
        match self.index(project) {
            Some(i) => {
                self.live[i].last_used = now;
                true
            }
            None => false,
        }
    }

    /// Открыть проект или вернуть уже живой процесс.
    ///
    /// `spawn` вызывается **только** при открытии: он поднимает воркер и делает
    /// `hello` + `start` (в т.ч. с `initialMessages` при возобновлении). Повторный
    /// вызов для того же проекта процесс не перезапускает и историю не
    /// подставляет — иначе `initialMessages` удвоили бы переписку (A0 §3.6).
    ///
    /// Перед открытием нового проекта реестр выгружает LRU-жертву, если живых
    /// процессов уже `max_live`. Упавший процесс (`is_closed`) переоткрывается.
    pub fn get_or_open<F>(
        &mut self,
        project: &str,
        now: Instant,
        spawn: F,
    ) -> Result<&mut Gateway, GatewayError>
    where
        F: FnOnce() -> Result<Gateway, GatewayError>,
    {
        if let Some(i) = self.index(project) {
            if !self.live[i].gateway.is_closed() {
                self.live[i].last_used = now;
                return Ok(&mut self.live[i].gateway);
            }
            // Процесс умер (сбой/внешний kill): запись бесполезна — выгружаем
            // и открываем заново ниже.
            self.unload(project);
        }
        while self.live.len() >= self.max_live {
            if self.evict_lru().is_none() {
                break;
            }
        }
        let gateway = spawn()?;
        self.live.push(Live {
            project: project.to_string(),
            gateway,
            last_used: now,
        });
        Ok(&mut self
            .live
            .last_mut()
            .expect("проект только что добавлен")
            .gateway)
    }

    /// Выгрузить проект: `stop` + `shutdown` (ожидание выхода ≤ 5 с), запись
    /// удаляется. `true`, если проект был открыт.
    pub fn unload(&mut self, project: &str) -> bool {
        let Some(i) = self.index(project) else {
            return false;
        };
        let entry = self.live.remove(i);
        shutdown(entry.gateway);
        true
    }

    /// Выгрузить проекты, простаивавшие дольше `idle`, и вернуть их
    /// идентификаторы. Свежие проекты не трогаются.
    pub fn reap_idle(&mut self, now: Instant) -> Vec<String> {
        let stale: Vec<String> = self
            .live
            .iter()
            .filter(|entry| now.saturating_duration_since(entry.last_used) > self.idle)
            .map(|entry| entry.project.clone())
            .collect();
        for project in &stale {
            self.unload(project);
        }
        stale
    }

    /// Выгрузить LRU-проект (тот, что дольше всех не использовался).
    fn evict_lru(&mut self) -> Option<String> {
        let (i, project) = self
            .live
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(i, entry)| (i, entry.project.clone()))?;
        let entry = self.live.remove(i);
        shutdown(entry.gateway);
        Some(project)
    }

    /// Индекс проекта (идентификатор сравнивается без учёта регистра, как в
    /// [`crate::projects::Projects::get`]).
    fn index(&self, project: &str) -> Option<usize> {
        self.live
            .iter()
            .position(|entry| entry.project.eq_ignore_ascii_case(project))
    }
}

impl Drop for AgentRegistry {
    fn drop(&mut self) {
        for entry in std::mem::take(&mut self.live) {
            shutdown(entry.gateway);
        }
    }
}

/// Мягкая остановка и выход процесса. Ошибки намеренно проглатываются: процесс
/// мог умереть сам, а `shutdown` в этом случае добивает его без ожидания.
fn shutdown(gateway: Gateway) {
    let _ = gateway.stop();
    gateway.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use serde_json::Value;

    use hds_anonymize::protocol::Snapshot;

    use crate::deanon::DeAnonymizer;
    use crate::env;
    use crate::filter::{sys_values, Filter};
    use crate::gateway::GatewayHost;
    use crate::protocol::Provider;
    use crate::settings::UnknownPolicy;
    use crate::worker::WorkerSpec;

    /// Крошечный воркер-заглушка (как `ECHO_WORKER` в `worker.rs`): отвечает на
    /// протокол §4.3 и пишет **свой PID, полученные команды и факт выхода** в
    /// файлы, поэтому тест видит жизненный цикл процесса, а не только ответы.
    ///
    /// `send {text: "__crash__"}` имитирует падение воркера.
    const STUB_WORKER: &str = r#"
import fs from 'node:fs';
import readline from 'node:readline';

const logFile = process.env.HDS_STUB_LOG;
const pidFile = process.env.HDS_STUB_PID;
const exitFile = process.env.HDS_STUB_EXIT;

fs.writeFileSync(pidFile, String(process.pid));
fs.appendFileSync(logFile, JSON.stringify({ type: 'spawn', pid: process.pid }) + '\n');
process.on('exit', () => {
  try { fs.writeFileSync(exitFile, String(process.pid)); } catch {}
});

const out = (message) => process.stdout.write(JSON.stringify(message) + '\n');
readline.createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', (line) => {
  const text = line.trim();
  if (!text.startsWith('{')) return;
  const msg = JSON.parse(text);
  fs.appendFileSync(logFile, text + '\n');
  if (msg.type === 'hello') {
    out({ type: 'ready', id: msg.id, protocol: 1 });
  } else if (msg.type === 'start') {
    out({ type: 'done', id: msg.id, ok: true, result: {
      session_id: msg.session_id ?? 'stub-session',
      declared_tools: ['read_files'],
    } });
  } else if (msg.type === 'send') {
    if (msg.text === '__crash__') process.exit(1);
    out({ type: 'done', id: msg.id, ok: true, result: { accepted: true } });
  } else if (msg.type === 'shutdown') {
    out({ type: 'done', id: msg.id, ok: true, result: {} });
    process.exit(0);
  } else {
    out({ type: 'done', id: msg.id, ok: true, result: {} });
  }
});
"#;

    fn node_available() -> bool {
        Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Окружение Node: продакшн-набор [`env::build`].
    fn child_env(config: &Path, state: &Path, workspace: &Path) -> Vec<(String, String)> {
        let cfg = serde_yaml::Value::Mapping(Default::default());
        env::build(&cfg, config, state, workspace, None).vars
    }

    /// Стенд: каталог с заглушкой, журналами и папками проектов.
    struct Stub {
        base: PathBuf,
    }

    impl Stub {
        fn new(tag: &str) -> Stub {
            // Тесты идут параллельно в одном процессе: каталог должен быть свой.
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let seq = SEQ.fetch_add(1, Ordering::SeqCst);
            let base = std::env::temp_dir().join(format!(
                "hds-agent-registry-{tag}-{}-{seq}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).expect("каталог стенда");
            fs::write(base.join("stub_worker.mjs"), STUB_WORKER).expect("stub_worker.mjs");
            Stub { base }
        }

        fn log(&self, project: &str) -> PathBuf {
            self.base.join(format!("{project}.commands.jsonl"))
        }

        /// Поднять шлюз проекта: `hello` + `start` без истории (как `spawn`
        /// в [`AgentRegistry::get_or_open`]).
        fn gateway(&self, project: &str) -> Result<Gateway, GatewayError> {
            let root = self.base.join(project);
            let workspace = root.join("workspace");
            let config = root.join("agent-config");
            let state = root.join("agent-state");
            env::prepare_dirs(&config, &state, &workspace).expect("каталоги проекта");
            let mut vars = child_env(&config, &state, &workspace);
            for (name, path) in [
                ("HDS_STUB_LOG", self.log(project)),
                ("HDS_STUB_PID", self.base.join(format!("{project}.pid"))),
                ("HDS_STUB_EXIT", self.base.join(format!("{project}.exit"))),
            ] {
                vars.push((name.to_string(), path.to_string_lossy().into_owned()));
            }
            let spec = WorkerSpec {
                program: PathBuf::from("node"),
                script: self.base.join("stub_worker.mjs"),
                cwd: workspace.clone(),
                env: vars,
                stderr_log: Some(self.base.join(format!("{project}.stderr.log"))),
                timeout: Duration::from_secs(30),
            };
            let snapshot = Snapshot::default();
            let filter = Arc::new(Filter::new(
                project,
                Vec::new(),
                UnknownPolicy::Label,
                Duration::from_secs(300),
                None,
            ));
            filter.set_snapshot(&snapshot, &[], &[], &[]);
            let deanon = Arc::new(std::sync::Mutex::new(DeAnonymizer::new(
                &snapshot,
                &sys_values(&[], &[]),
                &workspace,
                &root,
            )));
            let host = Arc::new(GatewayHost::without_client(
                project,
                &workspace,
                Arc::clone(&filter),
                Arc::new(crate::gateway::NoAskUser),
                None,
            ));
            let mut gateway = Gateway::spawn_egress_only(
                &spec, host, project, &workspace, &state, filter, deanon,
            )?;
            gateway.hello()?;
            gateway.open(stub_provider(), Vec::new(), None, 4, false)?;
            Ok(gateway)
        }

        /// Команды, полученные всеми процессами проекта.
        fn commands(&self, project: &str) -> Vec<Value> {
            fs::read_to_string(self.log(project))
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect()
        }

        /// Сколько раз проект получал `start` (один процесс — один `start`).
        fn starts(&self, project: &str) -> usize {
            self.commands(project)
                .iter()
                .filter(|command| command["type"] == "start")
                .count()
        }

        /// PID последнего запущенного процесса проекта.
        fn pid(&self, project: &str) -> u32 {
            fs::read_to_string(self.base.join(format!("{project}.pid")))
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(0)
        }

        /// Процесс с этим PID завершился (заглушка пишет файл выхода).
        fn exited(&self, project: &str, pid: u32) -> bool {
            fs::read_to_string(self.base.join(format!("{project}.exit")))
                .map(|text| text.trim() == pid.to_string())
                .unwrap_or(false)
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Провайдер-заглушка: сеть не нужна, запросов к модели в этих тестах нет.
    fn stub_provider() -> Provider {
        Provider {
            kind: "openai-compatible".to_string(),

            base_url: "http://127.0.0.1:9/v1".to_string(),
            model: "stub-model".to_string(),
            api_key: "sk-stub-key".to_string(),
        }
    }

    /// Границы по умолчанию — из `AgentSettings` (В-16: 2 проекта, 30 минут).
    #[test]
    fn defaults_come_from_settings() {
        let registry = AgentRegistry::defaults();
        assert_eq!(registry.max_live(), 2);
        assert_eq!(registry.idle(), Duration::from_secs(30 * 60));
        assert_eq!(DEFAULT_MAX_LIVE, 2);
        assert_eq!(DEFAULT_IDLE, Duration::from_secs(30 * 60));
        assert!(registry.is_empty());

        let settings = AgentSettings {
            max_live_projects: 3,
            idle_secs: 300,
            ..AgentSettings::default()
        };
        let registry = AgentRegistry::from_settings(&settings);
        assert_eq!(registry.max_live(), 3);
        assert_eq!(registry.idle(), Duration::from_secs(300));

        // Ноль живых процессов невозможен: `get_or_open` обязан вернуть шлюз.
        assert_eq!(AgentRegistry::new(0, DEFAULT_IDLE).max_live(), 1);
    }

    /// Операции над незарегистрированным проектом — no-op, без паники.
    #[test]
    fn missing_project_operations_are_noops() {
        let mut registry = AgentRegistry::new(2, DEFAULT_IDLE);
        let now = Instant::now();
        assert!(!registry.unload("P9999"));
        assert!(registry.get_mut("P9999").is_none());
        assert!(!registry.touch("P9999", now));
        assert!(registry
            .reap_idle(now + Duration::from_secs(3600))
            .is_empty());
        assert!(registry.live().is_empty());
        assert_eq!(registry.len(), 0);
    }

    /// Переиспользование: повторный `get_or_open` процесс не перезапускает.
    ///
    /// Один процесс = один `start`; `spawn` вызывается ровно один раз, PID тот
    /// же, отметка последнего использования обновилась. Повторного
    /// `initialMessages` нет по построению: `start` в журнале один.
    #[test]
    fn reuses_live_process_for_same_project() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест переиспользования пропущен");
            return;
        }
        let stub = Stub::new("reuse");
        let mut registry = AgentRegistry::new(2, Duration::from_secs(1800));
        let calls = Cell::new(0);
        let t0 = Instant::now();

        let pid = registry
            .get_or_open("P0001", t0, || {
                calls.set(calls.get() + 1);
                stub.gateway("P0001")
            })
            .expect("первый запуск")
            .pid();
        let t1 = t0 + Duration::from_secs(5);
        let again = registry
            .get_or_open("P0001", t1, || {
                calls.set(calls.get() + 1);
                stub.gateway("P0001")
            })
            .expect("переиспользование")
            .pid();

        assert_eq!(calls.get(), 1, "spawn не должен вызываться повторно");
        assert_eq!(pid, again, "процесс переиспользован");
        assert_eq!(stub.starts("P0001"), 1, "второго start быть не должно");
        assert_eq!(stub.pid("P0001"), pid);
        assert_eq!(registry.live(), vec![("P0001".to_string(), t1)]);
        assert!(!stub.exited("P0001", pid), "процесс должен быть жив");
    }

    /// `max_live = 1`: открытие второго проекта выгружает первый.
    #[test]
    fn max_live_one_unloads_first_project() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест max_live=1 пропущен");
            return;
        }
        let stub = Stub::new("max1");
        let mut registry = AgentRegistry::new(1, Duration::from_secs(1800));
        let t0 = Instant::now();
        let first = registry
            .get_or_open("P0001", t0, || stub.gateway("P0001"))
            .expect("P0001")
            .pid();

        let second = registry
            .get_or_open("P0002", t0 + Duration::from_secs(1), || {
                stub.gateway("P0002")
            })
            .expect("P0002")
            .pid();

        assert_ne!(first, second, "это разные процессы");
        assert_eq!(registry.projects(), vec!["P0002".to_string()]);
        assert!(registry.get_mut("P0001").is_none(), "запись ушла");
        assert!(
            stub.exited("P0001", first),
            "процесс P0001 обязан завершиться при выгрузке"
        );
        assert!(!stub.exited("P0002", second), "P0002 должен быть жив");
    }

    /// `max_live = 2`: оба проекта живы, второй не убивает первый.
    #[test]
    fn max_live_two_keeps_both_projects() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест max_live=2 пропущен");
            return;
        }
        let stub = Stub::new("max2");
        let mut registry = AgentRegistry::new(2, Duration::from_secs(1800));
        let t0 = Instant::now();
        let first = registry
            .get_or_open("P0001", t0, || stub.gateway("P0001"))
            .expect("P0001")
            .pid();
        let second = registry
            .get_or_open("P0002", t0 + Duration::from_secs(1), || {
                stub.gateway("P0002")
            })
            .expect("P0002")
            .pid();

        assert_ne!(first, second);
        assert_eq!(
            registry.projects(),
            vec!["P0001".to_string(), "P0002".to_string()]
        );
        assert!(!stub.exited("P0001", first), "P0001 не должен выгружаться");
        assert!(!stub.exited("P0002", second));
    }

    /// Жертва при переполнении — LRU: проект, который дольше не использовался.
    #[test]
    fn lru_victim_is_least_recently_used() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест LRU пропущен");
            return;
        }
        let stub = Stub::new("lru");
        let mut registry = AgentRegistry::new(2, Duration::from_secs(1800));
        let t0 = Instant::now();
        let oldest = registry
            .get_or_open("P0001", t0, || stub.gateway("P0001"))
            .expect("P0001")
            .pid();
        let middle = registry
            .get_or_open("P0002", t0 + Duration::from_secs(1), || {
                stub.gateway("P0002")
            })
            .expect("P0002")
            .pid();
        // P0001 использован позже P0002 → жертвой становится P0002.
        assert!(registry.touch("P0001", t0 + Duration::from_secs(2)));

        registry
            .get_or_open("P0003", t0 + Duration::from_secs(3), || {
                stub.gateway("P0003")
            })
            .expect("P0003");

        assert_eq!(
            registry.projects(),
            vec!["P0001".to_string(), "P0003".to_string()]
        );
        assert!(stub.exited("P0002", middle), "выгружен LRU-проект P0002");
        assert!(!stub.exited("P0001", oldest), "свежий P0001 остался");
    }

    /// `reap_idle`: выгружаются только проекты, бездействовавшие дольше `idle`;
    /// после выгрузки проект открывается заново (новый процесс).
    #[test]
    fn reap_idle_unloads_stale_and_reopens() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест простоя пропущен");
            return;
        }
        let stub = Stub::new("idle");
        let mut registry = AgentRegistry::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        let stale = registry
            .get_or_open("P0001", t0, || stub.gateway("P0001"))
            .expect("P0001")
            .pid();
        let fresh = registry
            .get_or_open("P0002", t0 + Duration::from_secs(40), || {
                stub.gateway("P0002")
            })
            .expect("P0002")
            .pid();

        let reaped = registry.reap_idle(t0 + Duration::from_secs(70));
        assert_eq!(reaped, vec!["P0001".to_string()]);
        assert_eq!(registry.projects(), vec!["P0002".to_string()]);
        assert!(
            stub.exited("P0001", stale),
            "простой проект обязан выгрузиться"
        );
        assert!(!stub.exited("P0002", fresh), "свежий проект не трогаем");

        // Выгруженный проект открывается заново — это новый процесс.
        let reopened = registry
            .get_or_open("P0001", t0 + Duration::from_secs(71), || {
                stub.gateway("P0001")
            })
            .expect("P0001 заново")
            .pid();
        assert_ne!(reopened, stale);
        assert_eq!(stub.starts("P0001"), 2, "новый процесс — новый start");
        assert!(!stub.exited("P0001", reopened));
    }

    /// Упавший воркер переоткрывается: запись живого процесса не переживает
    /// смерть процесса (иначе UI отдал бы мёртвый шлюз).
    #[test]
    fn dead_worker_is_reopened() {
        if !node_available() {
            eprintln!("реестр: node недоступен — тест падения пропущен");
            return;
        }
        let stub = Stub::new("crash");
        let mut registry = AgentRegistry::new(2, Duration::from_secs(1800));
        let t0 = Instant::now();
        let first = registry
            .get_or_open("P0001", t0, || stub.gateway("P0001"))
            .expect("P0001")
            .pid();

        let crashed = {
            let gateway = registry.get_mut("P0001").expect("шлюз");
            gateway.send_anonymized("__crash__")
        };
        assert!(crashed.is_err(), "падение воркера видно вызывающему");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !registry.get_mut("P0001").expect("шлюз").is_closed() {
            assert!(Instant::now() < deadline, "канал не закрылся за 10 с");
            std::thread::sleep(Duration::from_millis(20));
        }

        let second = registry
            .get_or_open("P0001", t0 + Duration::from_secs(1), || {
                stub.gateway("P0001")
            })
            .expect("переоткрытие")
            .pid();
        assert_ne!(first, second, "мёртвый процесс не переиспользуется");
        assert_eq!(stub.starts("P0001"), 2);
        assert!(stub.exited("P0001", first));
        assert!(!stub.exited("P0001", second));
    }
}
