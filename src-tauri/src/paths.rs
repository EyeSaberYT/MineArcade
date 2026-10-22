use std::path::PathBuf;

use anyhow::{Context, Result};

/// Layout of the launcher's data directory (%APPDATA%\MineArcade on Windows).
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let root = dirs::data_dir()
            .context("could not find the user data directory")?
            .join("MineArcade");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("could not create {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    pub fn instances_file(&self) -> PathBuf {
        self.root.join("instances.json")
    }

    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.root.join("versions").join(id)
    }

    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }

    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }

    /// The game directory of an instance (saves, mods, options.txt, ...).
    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.root.join("instances").join(id)
    }

    /// Native libraries are extracted per instance so two running games never fight over DLLs.
    pub fn natives_dir(&self, instance_id: &str) -> PathBuf {
        self.root.join("natives").join(instance_id)
    }
}
