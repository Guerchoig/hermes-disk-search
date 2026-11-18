//! Корни анонимной зоны (TR-4.1, D9): при `agent.enabled: true` эффективные
//! корни — только `<agent.root>\P<NNNN>\workspace`, а `index.roots` игнорируется.

use std::path::PathBuf;

use hds_core::config::Config;
use hds_core::roots::{
    agent_enabled, agent_root, agent_roots, effective_roots, is_agent_project, roots_from_cfg,
};

fn cfg(yaml: &str) -> Config {
    serde_yaml::from_str(yaml).unwrap()
}

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-aroots-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mkdir(p: &std::path::Path) {
    std::fs::create_dir_all(p).unwrap();
}

#[test]
fn project_names_are_p_plus_four_digits() {
    assert!(is_agent_project("P0001"));
    assert!(is_agent_project("P9999"));
    assert!(!is_agent_project("P001"));
    assert!(!is_agent_project("P00001"));
    assert!(!is_agent_project("p0001"));
    assert!(!is_agent_project("P000X"));
    assert!(!is_agent_project("agent-config"));
    assert!(!is_agent_project(""));
}

#[test]
fn effective_roots_switch_to_agent_zone() {
    let dir = tmpdir("switch");
    // Три проекта; у P0003 нет workspace → корнем не становится.
    mkdir(&dir.join("P0001/workspace/01 Устав"));
    mkdir(&dir.join("P0001/agent-config"));
    mkdir(&dir.join("P0002/workspace"));
    mkdir(&dir.join("P0003"));
    mkdir(&dir.join("not-a-project/workspace"));

    let yaml = format!(
        "index:\n  roots:\n    - 'C:/Users/somebody/Documents'\nagent:\n  enabled: true\n  root: '{}'\n",
        dir.to_string_lossy().replace('\\', "/")
    );
    let c = cfg(&yaml);
    assert!(agent_enabled(&c));
    assert!(agent_root(&c).is_some());

    let roots = effective_roots(&c);
    let projects: Vec<&str> = roots.iter().map(|r| r.project.as_str()).collect();
    assert_eq!(projects, vec!["P0001", "P0002"], "только P*\\workspace");
    assert!(roots[0]
        .path
        .replace('\\', "/")
        .ends_with("/P0001/workspace"));
    // agent-roots не равны index.roots
    assert_eq!(roots_from_cfg(&c).len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agent_disabled_keeps_index_roots() {
    let c =
        cfg("index:\n  roots:\n    - 'C:/data'\nagent:\n  enabled: false\n  root: 'D:/hds-anon'\n");
    assert!(!agent_enabled(&c));
    let roots = effective_roots(&c);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, "C:/data");
    assert!(agent_roots(&c).is_empty() || roots[0].path != "D:/hds-anon");
}

#[test]
fn missing_agent_root_gives_no_roots() {
    let dir = tmpdir("missing");
    let yaml = format!(
        "index:\n  roots:\n    - 'C:/data'\nagent:\n  enabled: true\n  root: '{}'\n",
        dir.join("nope").to_string_lossy().replace('\\', "/")
    );
    let c = cfg(&yaml);
    assert!(effective_roots(&c).is_empty(), "нет каталога — нет корней");
    let _ = std::fs::remove_dir_all(&dir);
}
