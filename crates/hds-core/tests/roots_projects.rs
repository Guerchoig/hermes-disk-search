//! Тесты корней → проектов (M1 T2, `PLAN_M1_ACCESS_PROJECTS.md` §3.1):
//! обе формы элементов, проект по умолчанию (включая корень диска `D:\` → `D:`),
//! выбор корня по наибольшему префиксу, файл вне корней → `None`.
//! Тесты БД (project в files, дроп старой схемы) — `db_project.rs`.

use hds_core::config::Config;
use hds_core::roots::{default_project, project_for_path, roots_from_cfg, Root};

fn cfg(yaml: &str) -> Config {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn roots_parse_string_and_object_forms() {
    let c = cfg(
        "index:\n  roots:\n    - \"E:\\\\Archive\"\n    - { path: \"D:\\\\Work\\\\Sales\", project: \"Sales\" }\n    - { path: \"F:\\\\Reports\", project: \"Sales\" }\n",
    );
    let roots = roots_from_cfg(&c);
    assert_eq!(
        roots,
        vec![
            Root {
                path: "E:\\Archive".into(),
                project: "Archive".into(),
            },
            Root {
                path: "D:\\Work\\Sales".into(),
                project: "Sales".into(),
            },
            Root {
                path: "F:\\Reports".into(),
                project: "Sales".into(),
            },
        ]
    );
}

#[test]
fn roots_parse_string_list_backcompat() {
    // Прежний формат — список строк (walker не сломан).
    let c = cfg("index:\n  roots:\n    - 'C:/data'\n    - 'D:/doc'\n");
    let roots = roots_from_cfg(&c);
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].project, "data");
    assert_eq!(roots[1].project, "doc");
}

#[test]
fn roots_empty_and_missing() {
    assert!(roots_from_cfg(&cfg("index: {roots: []}")).is_empty());
    assert!(roots_from_cfg(&cfg("other: 1")).is_empty());
}

#[test]
fn default_project_names() {
    // Корень диска — метка с двоеточием (решение заказчика 07.10.2026).
    assert_eq!(default_project("D:\\"), "D:");
    assert_eq!(default_project("D:"), "D:");
    assert_eq!(default_project("D:/"), "D:");
    // Младшая папка.
    assert_eq!(default_project("E:\\Archive"), "Archive");
    assert_eq!(default_project("E:/Archive/Reports"), "Reports");
    // UNC — последний компонент.
    assert_eq!(default_project("\\\\srv\\share"), "share");
}

#[test]
fn project_by_longest_prefix() {
    let roots = roots_from_cfg(&cfg(
        "index:\n  roots:\n    - \"D:\\\\\"\n    - { path: \"D:\\\\Work\\\\Sales\", project: \"Sales\" }\n",
    ));
    // Наибольший префикс побеждает, регистр не важен.
    assert_eq!(
        project_for_path(&roots, "D:\\Work\\Sales\\q1.xlsx"),
        Some("Sales")
    );
    assert_eq!(
        project_for_path(&roots, "d:/work/sales/x.txt"),
        Some("Sales")
    );
    // Другой подкаталог диска → корневой проект диска.
    assert_eq!(project_for_path(&roots, "D:\\Documents\\a.pdf"), Some("D:"));
    // Вне корней → None (в server-режиме не виден).
    assert_eq!(project_for_path(&roots, "E:\\Other\\a.txt"), None);
}

#[test]
fn same_project_on_two_roots() {
    let roots = roots_from_cfg(&cfg(
        "index:\n  roots:\n    - { path: \"D:\\\\Work\\\\Sales\", project: \"Sales\" }\n    - { path: \"F:\\\\Reports\", project: \"Sales\" }\n",
    ));
    assert_eq!(
        project_for_path(&roots, "F:\\Reports\\2025\\r.md"),
        Some("Sales")
    );
    assert_eq!(
        project_for_path(&roots, "D:\\Work\\Sales\\x.txt"),
        Some("Sales")
    );
}
