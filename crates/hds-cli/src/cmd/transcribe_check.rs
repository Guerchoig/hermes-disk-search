//! `hds transcribe-check` (alias `whisper-check`) — проверка стека транскрибации.
//!
//! P1: показывает выбранный движок (`transcribe.engine`), профиль, интерпретатор
//! воркера, `probe` Python-стека (torch/CUDA, ORT EP, ffmpeg, кэши моделей) и —
//! при `--file` — **живую** транскрибацию тем же путём, что боевой (`python` или
//! `legacy`, решает `TranscribeClient`).
//!
//! Отличие от `whisper-check` (W3): модель теперь GigaAM/pyannote, а не whisper
//! GGML; старые ключи `index.whisper_*` работают только при `engine: legacy`.

use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime};

use hds_core::config::{load, project_root, Config};
use hds_index::transcribe::{TranscribeClient, TranscribeConfig};
use hds_transcribe::{Client, ClientConfig, Settings};

/// `cmd_transcribe_check(file, json)`: 0 — стек на месте и (при `--file`)
/// транскрибация успешна; 1 — иначе.
pub fn cmd_transcribe_check(file: Option<String>, json: bool) -> i32 {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    cmd_transcribe_check_cfg(&cfg, file, json)
}

/// То же с явным конфигом (тесты не трогают боевой `config.yaml`).
pub fn cmd_transcribe_check_cfg(cfg: &Config, file: Option<String>, json: bool) -> i32 {
    let settings = Settings::from_config(cfg);
    let root = project_root();
    let python = settings.python(&root);

    if let Some(file) = file {
        let client = TranscribeClient::new(TranscribeConfig::from_config(cfg));
        let t0 = Instant::now();
        let result = client.transcribe(Path::new(&file));
        let elapsed = t0.elapsed().as_secs_f64();
        return match result {
            Ok(segments) => {
                if json {
                    let segs: Vec<serde_json::Value> = segments
                        .iter()
                        .map(|s| {
                            serde_json::json!({"text": s.text, "t_start": s.t_start, "t_end": s.t_end})
                        })
                        .collect();
                    let out = serde_json::json!({
                        "engine": settings.engine,
                        "profile": settings.profile,
                        "file": file,
                        "ok": true,
                        "segments": segs,
                        "elapsed_sec": elapsed,
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                } else {
                    print_static(cfg, &settings, python.as_deref());
                    println!(
                        "[ok] транскрибация «{}»: {} сегментов за {:.2} с",
                        file,
                        segments.len(),
                        elapsed
                    );
                }
                0
            }
            Err(e) => {
                if json {
                    let out = serde_json::json!({
                        "engine": settings.engine,
                        "profile": settings.profile,
                        "file": file,
                        "ok": false,
                        "error": e.message(),
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                } else {
                    eprintln!("[!!] транскрибация: {}", e.message());
                }
                1
            }
        };
    }

    // Статическая проверка: Python-стек (probe) — без падения на отсутствии venv.
    // В legacy-режиме интерпретатор не нужен: проверка не должна «падать» только
    // из-за отсутствия venv (сценарий отката §12).
    let ok = if settings.is_python() {
        python.is_some()
    } else {
        true
    };
    let probe = if settings.is_python() {
        python
            .as_ref()
            .and_then(|py| probe_worker(py, &root, &settings))
    } else {
        None
    };
    // P2: агрегат `alignment_quality` последних заданий — диагностика флагов без UI.
    let transcripts = latest_transcripts(&settings.state_dir, 3);
    if json {
        let rows: Vec<serde_json::Value> = transcripts.iter().map(transcript_json).collect();
        let out = serde_json::json!({
            "engine": settings.engine,
            "profile": settings.profile,
            "python": python.as_ref().map(|p| p.display().to_string()),
            "ok": ok,
            "probe": probe,
            "alignment": alignment_json(&settings),
            "glossary": glossary_json(&settings),
            "hotwords": hotwords_json(&settings),
            "llm": llm_json(&settings),
            "diarization": {
                "enabled": settings.diarization.enabled,
                "model": settings.diarization.model,
                "device": settings.diarization.device,
                "block_minutes": settings.diarization.block_minutes,
            },
            "transcripts": rows,
            "notices": settings.notices,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        print_static(cfg, &settings, python.as_deref());
        print_probe(probe.as_ref());
        print_transcripts(&transcripts);
    }
    if ok {
        0
    } else {
        1
    }
}

/// `probe` воркера (версии/EP/кэши); ошибки не роняют проверку.
fn probe_worker(py: &Path, root: &Path, settings: &Settings) -> Option<serde_json::Value> {
    let mut ccfg = ClientConfig::from_settings(py, root, settings);
    ccfg.stderr_log = Some(std::env::temp_dir().join("hds-transcribe-check.log"));
    let mut client = Client::spawn(ccfg).ok()?;
    let value = client.probe().ok();
    client.shutdown();
    value
}

/// Текстовый отчёт о выбранном пути (без запуска воркера).
fn print_static(_cfg: &Config, settings: &Settings, python: Option<&Path>) {
    println!(
        "[..] транскрибация: engine={} (профиль {})",
        settings.engine, settings.profile
    );
    match python {
        Some(p) => println!("[ok] интерпретатор воркера: {}", p.display()),
        None => println!(
            "[--] интерпретатор воркера не найден (HDS_TRANSCRIBE_PYTHON / \
             sidecar\\venv-transcribe[-dml] / sidecar\\python)"
        ),
    }
    let ffmpeg = if settings.decode.ffmpeg.is_empty() {
        "PATH".to_string()
    } else {
        settings.decode.ffmpeg.clone()
    };
    println!("[..] ffmpeg: {ffmpeg}");
    if settings.is_python() {
        println!(
            "[..] ASR: {} / {} / device {} (batch {})",
            settings.asr.variant,
            settings.asr.backend,
            settings.asr.device,
            settings.asr.batch_size
        );
        println!(
            "[..] диаризация: {} (device {}, блоки {:.0} мин)",
            settings.diarization.model,
            settings.diarization.device,
            settings.diarization.block_minutes
        );
        let quality = &settings.align.quality;
        println!(
            "[..] alignment: mode={} (max_shift {:.2} с, gap {:.2} с, min_dur {:.2} с, \
             overlap {:.2} с, unreliable ≤ {:.0} %)",
            settings.align.mode,
            settings.align.max_shift_sec,
            quality.max_gap_sec,
            quality.min_duration_sec,
            quality.overlap_tolerance_sec,
            quality.max_unreliable_ratio * 100.0
        );
        // P3: глоссарий и состояние hotwords — без UI (причины/лимиты в журнале).
        let path = settings.glossary.path.trim();
        let state = if path.is_empty() {
            "не задан (замены выключены)".to_string()
        } else if Path::new(path).is_file() {
            format!(
                "файл есть (терминов в glossary.terms конфига: {})",
                settings.glossary.terms.len()
            )
        } else {
            "файла нет (замены выключены)".to_string()
        };
        println!(
            "[{}] глоссарий: {} — {}",
            if path.is_empty() || !Path::new(path).is_file() {
                "--"
            } else {
                "ok"
            },
            if path.is_empty() { "—" } else { path },
            state
        );
        let hot = &settings.hotwords;
        println!(
            "[{}] hotwords: backend={} (enabled={}, фраз {}, boost {:.1}, лимит {}×{}, \
             ворота G1/G2 {})",
            if hot.backend == "none" { "--" } else { "ok" },
            hot.backend,
            hot.enabled,
            hot.phrases.len(),
            hot.boost,
            hot.max_phrases,
            hot.max_phrase_chars,
            if hds_transcribe::settings::HOTWORDS_GATES_PASSED {
                "пройдены"
            } else {
                "не пройдены"
            }
        );
        // P4: LLM-постобработка (патчи через llm_host). По умолчанию выключена.
        let llm = &settings.llm;
        println!(
            "[{}] LLM-постобработка: enabled={} model={} host={} (temperature 0, \
             ≤{} правок/1000 слов, max_tokens {}, timeout {:.0} с)",
            if llm.enabled { "ok" } else { "--" },
            llm.enabled,
            llm.model,
            llm.base_url,
            llm.max_edits_per_1000,
            llm.max_tokens,
            llm.timeout_sec
        );
    } else {
        println!("[..] engine=legacy: POST /internal/transcribe (владелец GPU — llm_host)");
    }
    for notice in &settings.notices {
        println!("     [!] {notice}");
    }
}

/// Печать `probe` (версии/устройства/модели) в формате `[ok]/[--]/[..]`.
fn print_probe(probe: Option<&serde_json::Value>) {
    let Some(probe) = probe else {
        println!("[--] probe воркера недоступен (venv профиля не собран?)");
        return;
    };
    if let Some(versions) = probe.get("versions") {
        println!(
            "[ok] версии: torch {} · gigaam {} · pyannote {} · onnxruntime {}",
            versions.get("torch").and_then(|v| v.as_str()).unwrap_or(""),
            versions
                .get("gigaam")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            versions
                .get("pyannote")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            versions
                .get("onnxruntime")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
    }
    if let Some(torch) = probe.get("devices").and_then(|d| d.get("torch")) {
        if let Some(err) = torch.get("error").and_then(|v| v.as_str()) {
            println!("[--] torch: {err}");
        } else {
            println!(
                "[{}] CUDA: {} ({})",
                if torch
                    .get("cuda_available")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "ok"
                } else {
                    "--"
                },
                torch
                    .get("device_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                torch
                    .get("cuda_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
        }
    }
    if let Some(ort) = probe.get("devices").and_then(|d| d.get("onnxruntime")) {
        let providers = ort
            .get("providers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!("[..] ORT EP: {providers}");
    }
    for key in ["ffmpeg", "gigaam_cache", "onnx_dir", "hf_home"] {
        if let Some(value) = probe.get(key).and_then(|v| v.as_str()) {
            println!("[..] {key}: {value}");
        }
    }
    let token = probe
        .get("hf_token")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "[{}] HF-токен: {}",
        if token { "ok" } else { "--" },
        if token {
            "задан"
        } else {
            "не задан (нужен только на первичную загрузку)"
        }
    );
    let cached = probe
        .get("diarization_model_cached")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "[{}] модель диаризации в кэше HF",
        if cached { "ok" } else { "--" }
    );
}

/// Сводка `alignment_quality` одного `*.transcript.json` (P2).
///
/// Нужна, чтобы причины флагов (`reasons`) и фактический режим alignment были
/// видны **без** UI: `hds transcribe-check` читает последние артефакты из
/// `transcribe.state_dir`. Слова/`alignment_reason` в `.md` не попадают
/// (контракт), поэтому это самый дешёвый способ увидеть агрегат.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptSummary {
    /// Имя транскрипта без суффикса `.transcript.json`.
    pub file: String,
    /// Имя медиафайла (без пути).
    pub media: String,
    /// Фактически использованный режим (`alignment_quality.mode_used`).
    pub mode: String,
    pub words: u64,
    pub blocks: u64,
    pub aligned_ratio: f64,
    pub low_confidence: u64,
    pub failed: u64,
    pub unreliable_ratio: f64,
    pub max_gap_sec: f64,
    /// `reasons`: (причина, число слов).
    pub reasons: Vec<(String, u64)>,
    pub rtf_total: Option<f64>,
    pub vram_peak_mb: Option<f64>,
    /// P3: слов-кандидатов на ручную проверку (`postprocess.needs_review`).
    pub review: u64,
    /// P4: была ли включена LLM, модель и счётчики патчей последнего прогона.
    pub llm_enabled: bool,
    pub llm_model: String,
    pub llm_applied: u64,
    pub llm_rejected: u64,
    pub llm_needs_review: u64,
    pub llm_latency_ms: Option<f64>,
}

fn number(value: Option<&serde_json::Value>) -> Option<f64> {
    value.and_then(|v| v.as_f64())
}

fn count(value: Option<&serde_json::Value>) -> u64 {
    number(value).unwrap_or(0.0).max(0.0) as u64
}

/// Разобрать `transcript.json`; `None` — файл не читается или нет `alignment_quality`.
pub fn summarize_transcript(path: &Path) -> Option<TranscriptSummary> {
    let text = fs::read_to_string(path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&text).ok()?;
    let quality = data.get("alignment_quality")?;
    let reasons = quality
        .get("reasons")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), count(Some(value))))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let file = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .trim_end_matches(".transcript.json")
        .to_string();
    let media = data
        .get("media")
        .and_then(|v| v.as_str())
        .map(|raw| {
            Path::new(raw)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(raw)
                .to_string()
        })
        .unwrap_or_default();
    Some(TranscriptSummary {
        file,
        media,
        mode: quality
            .get("mode_used")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        words: count(quality.get("words")),
        blocks: data
            .get("blocks")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0) as u64,
        aligned_ratio: number(quality.get("aligned_ratio")).unwrap_or(0.0),
        low_confidence: count(quality.get("low_confidence")),
        failed: count(quality.get("failed")),
        unreliable_ratio: number(quality.get("unreliable_ratio")).unwrap_or(0.0),
        max_gap_sec: number(quality.get("max_gap_sec")).unwrap_or(0.0),
        reasons,
        rtf_total: number(data.get("stats").and_then(|s| s.get("rtf_total"))),
        vram_peak_mb: number(data.get("stats").and_then(|s| s.get("vram_peak_mb"))),
        review: data
            .get("postprocess")
            .and_then(|p| p.get("needs_review"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0) as u64,
        // P4: счётчики LLM-патчей (`postprocess.llm`, пишет Python-воркер).
        llm_enabled: data
            .pointer("/postprocess/llm/enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        llm_model: data
            .pointer("/postprocess/llm/model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        llm_applied: count(data.pointer("/postprocess/llm/applied")),
        llm_rejected: count(data.pointer("/postprocess/llm/rejected")),
        llm_needs_review: count(data.pointer("/postprocess/llm/needs_review")),
        llm_latency_ms: number(data.pointer("/postprocess/llm/latency_ms")),
    })
}

/// Последние `*.transcript.json` в `state_dir` (свежие первыми), до `limit`.
pub fn latest_transcripts(dir: &Path, limit: usize) -> Vec<TranscriptSummary> {
    let mut rows: Vec<(SystemTime, TranscriptSummary)> = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !name.ends_with(".transcript.json") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(summary) = summarize_transcript(&path) {
            rows.push((modified, summary));
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    rows.into_iter()
        .take(limit)
        .map(|(_, summary)| summary)
        .collect()
}

/// Настройки alignment для `--json` (`hds transcribe-check`).
fn alignment_json(settings: &Settings) -> serde_json::Value {
    let quality = &settings.align.quality;
    serde_json::json!({
        "mode": settings.align.mode,
        "model": settings.align.model,
        "device": settings.align.device,
        "max_shift_sec": settings.align.max_shift_sec,
        "quality": {
            "max_gap_sec": quality.max_gap_sec,
            "min_duration_sec": quality.min_duration_sec,
            "overlap_tolerance_sec": quality.overlap_tolerance_sec,
            "min_aligned_ratio": quality.min_aligned_ratio,
            "max_unreliable_ratio": quality.max_unreliable_ratio,
        },
    })
}

/// Глоссарий (P3): путь, наличие файла, число терминов из конфига.
fn glossary_json(settings: &Settings) -> serde_json::Value {
    let path = settings.glossary.path.trim();
    serde_json::json!({
        "path": path,
        "exists": !path.is_empty() && Path::new(path).is_file(),
        "terms": settings.glossary.terms.len(),
    })
}

/// Hotwords (P3.6): backend, включённость, лимиты и статус ворот G1/G2.
fn hotwords_json(settings: &Settings) -> serde_json::Value {
    let hot = &settings.hotwords;
    serde_json::json!({
        "backend": hot.backend,
        "enabled": hot.enabled,
        "phrases": hot.phrases.len(),
        "max_phrases": hot.max_phrases,
        "max_phrase_chars": hot.max_phrase_chars,
        "boost": hot.boost,
        "url": hot.url,
        "gates_passed": hds_transcribe::settings::HOTWORDS_GATES_PASSED,
    })
}

/// LLM-постобработка (P4): включённость, модель, хост и лимиты патчей.
fn llm_json(settings: &Settings) -> serde_json::Value {
    let llm = &settings.llm;
    serde_json::json!({
        "enabled": llm.enabled,
        "base_url": llm.base_url,
        "model": llm.model,
        "temperature": llm.temperature,
        "max_tokens": llm.max_tokens,
        "timeout_sec": llm.timeout_sec,
        "max_edits_per_1000": llm.max_edits_per_1000,
    })
}

/// Одна сводка транскрипта для `--json`.
fn transcript_json(row: &TranscriptSummary) -> serde_json::Value {
    let reasons: serde_json::Map<String, serde_json::Value> = row
        .reasons
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::json!(value)))
        .collect();
    serde_json::json!({
        "file": row.file,
        "media": row.media,
        "mode": row.mode,
        "words": row.words,
        "blocks": row.blocks,
        "aligned_ratio": row.aligned_ratio,
        "low_confidence": row.low_confidence,
        "failed": row.failed,
        "unreliable_ratio": row.unreliable_ratio,
        "max_gap_sec": row.max_gap_sec,
        "reasons": reasons,
        "rtf_total": row.rtf_total,
        "vram_peak_mb": row.vram_peak_mb,
        "review": row.review,
        "llm": {
            "enabled": row.llm_enabled,
            "model": row.llm_model,
            "applied": row.llm_applied,
            "rejected": row.llm_rejected,
            "needs_review": row.llm_needs_review,
            "latency_ms": row.llm_latency_ms,
        },
    })
}

/// Печать сводок последних транскрипций (текстовый режим).
fn print_transcripts(rows: &[TranscriptSummary]) {
    if rows.is_empty() {
        println!("[--] последние транскрипции: в state_dir нет *.transcript.json");
        return;
    }
    println!("[..] последние транскрипции (state_dir):");
    for row in rows {
        let reasons = row
            .reasons
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(",");
        let rtf = row
            .rtf_total
            .map(|v| format!("{v:.4}"))
            .unwrap_or_else(|| "—".to_string());
        let vram = row
            .vram_peak_mb
            .map(|v| format!("{v:.0} МБ"))
            .unwrap_or_else(|| "—".to_string());
        let llm = if row.llm_enabled {
            format!(
                "{}: applied={} rejected={} review={} ({} мс)",
                row.llm_model,
                row.llm_applied,
                row.llm_rejected,
                row.llm_needs_review,
                row.llm_latency_ms
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "—".to_string())
            )
        } else {
            "off".to_string()
        };
        println!(
            "     {} ({}) · mode={} · words={} blocks={} · aligned={:.2}% · \
             low={} failed={} ({:.2}%) · gap≤{:.2} с · reasons={} · rtf_total={} · vram={} · \
             review={} · llm={}",
            row.file,
            row.media,
            row.mode,
            row.words,
            row.blocks,
            row.aligned_ratio * 100.0,
            row.low_confidence,
            row.failed,
            row.unreliable_ratio * 100.0,
            row.max_gap_sec,
            if reasons.is_empty() {
                "—"
            } else {
                reasons.as_str()
            },
            rtf,
            vram,
            row.review,
            llm
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).expect("создать файл");
        file.write_all(body.as_bytes()).expect("записать");
        path
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hds-tc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        dir
    }

    #[test]
    fn summarize_reads_alignment_quality() {
        let dir = temp_dir("summary");
        let path = write_file(
            &dir,
            "meet0924.transcript.json",
            r#"{
                "media": "D:/media/src_0924.mp3",
                "blocks": [{"text": "а"}, {"text": "б"}],
                "alignment_quality": {
                    "words": 2392, "aligned_ratio": 0.9954,
                    "low_confidence": 11, "failed": 0, "unreliable_ratio": 0.0046,
                    "max_gap_sec": 0.8, "mode_used": "native",
                    "reasons": {"gap": 11}
                },
                "stats": {"rtf_total": 0.0417, "vram_peak_mb": 6749.0},
                "postprocess": {
                    "needs_review": [{"word_id": 5, "term": "1С"}],
                    "llm": {"enabled": true, "model": "qwen2.5-7b-instruct",
                            "applied": 2, "rejected": 1, "needs_review": 1,
                            "latency_ms": 1234.5}
                }
            }"#,
        );
        let summary = summarize_transcript(&path).expect("разбор");
        assert_eq!(summary.file, "meet0924");
        assert_eq!(summary.media, "src_0924.mp3");
        assert_eq!(summary.mode, "native");
        assert_eq!(summary.words, 2392);
        assert_eq!(summary.blocks, 2);
        assert!((summary.aligned_ratio - 0.9954).abs() < 1e-9);
        assert_eq!(summary.low_confidence, 11);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.reasons, vec![("gap".to_string(), 11)]);
        assert_eq!(summary.rtf_total, Some(0.0417));
        assert_eq!(summary.vram_peak_mb, Some(6749.0));
        assert_eq!(summary.review, 1);
        // P4: счётчики патчей и модель из postprocess.llm.
        assert!(summary.llm_enabled);
        assert_eq!(summary.llm_model, "qwen2.5-7b-instruct");
        assert_eq!(summary.llm_applied, 2);
        assert_eq!(summary.llm_rejected, 1);
        assert_eq!(summary.llm_needs_review, 1);
        assert_eq!(summary.llm_latency_ms, Some(1234.5));

        let rows = latest_transcripts(&dir, 3);
        assert!(rows.iter().any(|row| row.file == "meet0924"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_skips_foreign_files_and_bad_json() {
        let dir = temp_dir("bad");
        write_file(&dir, "broken.transcript.json", "{not json");
        write_file(&dir, "notes.md", "# текст");
        assert!(latest_transcripts(&dir, 3).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glossary_and_hotwords_json_reflect_settings() {
        let cfg: hds_core::config::Config = serde_yaml::from_str(
            "transcribe:\n  glossary:\n    path: sidecar/hds_transcribe/glossary.example.yaml\n  hotwords:\n    enabled: true\n    backend: gigastt\n",
        )
        .expect("конфиг");
        let settings = Settings::from_config(&cfg);
        let glossary = glossary_json(&settings);
        assert_eq!(glossary["exists"], serde_json::json!(true));
        assert_eq!(glossary["terms"], 0);
        // backend принудительно none: ворота G1/G2 не пройдены (P3).
        let hotwords = hotwords_json(&settings);
        assert_eq!(hotwords["backend"], "none");
        assert_eq!(hotwords["enabled"], serde_json::json!(false));
        assert_eq!(hotwords["max_phrases"], 64);
        assert_eq!(hotwords["gates_passed"], serde_json::json!(false));
    }

    #[test]
    fn alignment_json_contains_quality_thresholds() {
        let settings = Settings::from_config(&hds_core::config::Config::Null);
        let value = alignment_json(&settings);
        assert_eq!(value["mode"], "native");
        assert_eq!(value["quality"]["overlap_tolerance_sec"], 0.12);
        assert_eq!(value["quality"]["max_unreliable_ratio"], 0.02);
    }
}
