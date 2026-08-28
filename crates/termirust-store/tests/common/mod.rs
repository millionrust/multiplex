#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use termirust_store::{PresetRepository, ProjectRepository, SessionRepository};

pub struct StoreFixture {
    pub temp: tempfile::TempDir,
    pub metadata: PathBuf,
}

impl StoreFixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let metadata = temp.path().join("metadata");
        ProjectRepository::open(&metadata).unwrap();
        SessionRepository::open(&metadata, temp.path().join("sessions")).unwrap();
        PresetRepository::open(&metadata).unwrap();
        Self { temp, metadata }
    }

    pub fn authoritative_bytes(&self) -> BTreeMap<String, Vec<u8>> {
        [
            "format.json",
            "projects.json",
            "sessions.json",
            "presets.json",
        ]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                fs::read(self.metadata.join(name)).unwrap(),
            )
        })
        .collect()
    }

    pub fn derived_path(&self, name: &str) -> PathBuf {
        self.metadata.join("derived-indexes").join(name)
    }
}

pub fn assert_no_repair_debris(root: &Path) {
    let indexes = root.join("derived-indexes");
    if !indexes.exists() {
        return;
    }
    for entry in fs::read_dir(indexes).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(".repair-"), "repair debris: {name}");
        assert_ne!(name, "repair-journal-v1.json");
    }
}
