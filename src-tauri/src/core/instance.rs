//! Instance model, persistence and the on-disk instance store.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::paths::{read_json, write_json, AppPaths};
use crate::error::{LauncherError, Result};

/// Supported mod loaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoaderKind::Vanilla => "vanilla",
            LoaderKind::Fabric => "fabric",
            LoaderKind::Quilt => "quilt",
            LoaderKind::Forge => "forge",
            LoaderKind::NeoForge => "neoforge",
        }
    }

    /// The loader name Modrinth uses in its API facets.
    pub fn modrinth_name(&self) -> &'static str {
        match self {
            LoaderKind::Vanilla => "vanilla",
            LoaderKind::Fabric => "fabric",
            LoaderKind::Quilt => "quilt",
            LoaderKind::Forge => "forge",
            LoaderKind::NeoForge => "neoforge",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "vanilla" | "release" => Some(LoaderKind::Vanilla),
            "fabric" => Some(LoaderKind::Fabric),
            "quilt" => Some(LoaderKind::Quilt),
            "forge" => Some(LoaderKind::Forge),
            "neoforge" => Some(LoaderKind::NeoForge),
            _ => None,
        }
    }
}

/// Where to send the player when the game starts (1.20+ "Quick Play").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum QuickPlay {
    /// Join a specific multiplayer server.
    Multiplayer { address: String },
    /// Load a singleplayer world, optionally by folder name.
    Singleplayer { world: String },
    /// Continue the most recently played world.
    SingleplayerContinue,
    /// Open the vanilla title screen (default).
    None,
}

impl Default for QuickPlay {
    fn default() -> Self {
        QuickPlay::None
    }
}

/// Per-instance launch and behaviour settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSettings {
    pub memory_min: u32,
    pub memory_max: u32,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default)]
    pub game_args: Vec<String>,
    /// Explicit Java executable for this instance.
    pub java_path: Option<String>,
    /// Override the game directory (defaults to the instance folder itself).
    pub game_dir: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    #[serde(default)]
    pub quick_play: QuickPlay,
    /// Extra environment variables passed to the game process.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl Default for InstanceSettings {
    fn default() -> Self {
        Self {
            memory_min: 1024,
            memory_max: 4096,
            jvm_args: vec![],
            game_args: vec![],
            java_path: None,
            game_dir: None,
            width: 1280,
            height: 720,
            fullscreen: false,
            quick_play: QuickPlay::None,
            env: vec![],
        }
    }
}

/// A fully described launcher instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub name: String,
    /// Emoji, short text or a cached icon filename.
    pub icon: Option<String>,
    pub game_version: String,
    #[serde(default)]
    pub loader: LoaderKind,
    /// Pinned loader build; `None` means "latest compatible".
    pub loader_version: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created: String,
    pub last_played: Option<String>,
    #[serde(default)]
    pub play_time_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub settings: InstanceSettings,
    /// Modrinth project ids installed through the launcher. Used for updates.
    #[serde(default)]
    pub installed_projects: Vec<InstalledProject>,
}

/// A single piece of content installed via Modrinth, tracked for updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledProject {
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    /// `mod`, `resourcepack`, `shader`, `datapack`, `modpack`
    pub project_type: String,
    /// Filenames that belong to this version so we can remove old files.
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub installed_at: Option<String>,
}

impl Instance {
    /// Build a fresh instance with sensible defaults.
    pub fn new(name: impl Into<String>, game_version: impl Into<String>, loader: LoaderKind) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            icon: None,
            game_version: game_version.into(),
            loader,
            loader_version: None,
            group: None,
            tags: vec![],
            created: chrono::Utc::now().to_rfc3339(),
            last_played: None,
            play_time_secs: 0,
            notes: None,
            settings: InstanceSettings::default(),
            installed_projects: vec![],
        }
    }

    /// Drop tracking records whose project id could never resolve.
    ///
    /// Early builds recorded a modpack's free-form `versionId` from the
    /// mrpack index (something like `26.1.2-1.1`) as if it were a Modrinth
    /// project id, so every file in the pack resolved to one junk record
    /// titled with that string. Modrinth ids are base62, so anything else
    /// can never resolve; the junk record's file list is salvaged onto the
    /// pack's own record so the content keeps its attribution.
    ///
    /// Returns whether anything changed.
    pub fn prune_invalid_projects(&mut self) -> bool {
        let valid = |id: &str| {
            id.is_empty()
                || id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };

        let mut salvaged: Vec<String> = vec![];
        self.installed_projects.retain_mut(|record| {
            if valid(&record.project_id) {
                return true;
            }
            salvaged.append(&mut record.files);
            false
        });

        if salvaged.is_empty() {
            return false;
        }

        // The pack record is the only honest owner of those files. Without
        // one the files are simply untracked: they stay on disk and the
        // content list falls back to their filenames.
        if let Some(pack) = self
            .installed_projects
            .iter_mut()
            .find(|record| record.project_type == "modpack")
        {
            for file in salvaged {
                if !pack.files.contains(&file) {
                    pack.files.push(file);
                }
            }
        }
        true
    }

    /// Take the fields the settings page edits from `edited`, keeping
    /// everything the backend owns.
    ///
    /// The frontend saves a copy it loaded when the page opened, so trusting
    /// it wholesale would roll back play time, the last-played stamp, loader
    /// installs and content tracked since then. The version and loader are
    /// changed through their own commands, never through a settings save.
    pub fn apply_user_edits(&mut self, edited: Instance) {
        self.name = edited.name;
        self.icon = edited.icon;
        self.group = edited.group;
        self.notes = edited.notes;
        self.settings = edited.settings;
    }
}

/// Generate a human-friendly, filesystem-safe instance id.
pub fn new_id() -> String {
    let short = Uuid::new_v4().simple().to_string();
    short[..10].to_string()
}

/// Create a URL/filesystem slug from a display name.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else {
        trimmed
    }
}

/// The index of every known instance, cached in memory.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstanceStore {
    #[serde(default)]
    pub instances: Vec<Instance>,
}

impl InstanceStore {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        let index = paths.instances_index();
        let mut store: InstanceStore = match read_json(&index) {
            Ok(store) => store.unwrap_or_default(),
            // A damaged index must not keep the launcher from starting: every
            // instance also has its own `instance.json`, so `reconcile` rebuilds
            // the list from the folders. The broken file is set aside first so
            // the next save doesn't destroy whatever it still holds.
            Err(error) => {
                let aside = index.with_extension("json.corrupt");
                log::warn!(
                    "instances: {} is unreadable ({error}); rebuilding from instance folders, \
                     original kept as {}",
                    index.display(),
                    aside.display()
                );
                if let Err(error) = std::fs::rename(&index, &aside) {
                    log::warn!("instances: could not set the broken index aside: {error}");
                }
                InstanceStore::default()
            }
        };
        store.sort();

        // Repair tracking records written by builds with the modpack id bug.
        let mut repaired = false;
        for instance in &mut store.instances {
            repaired |= instance.prune_invalid_projects();
        }
        if repaired {
            store.save(paths)?;
        }
        Ok(store)
    }

    fn sort(&mut self) {
        self.instances.sort_by(|a, b| {
            b.last_played
                .clone()
                .unwrap_or_else(|| b.created.clone())
                .cmp(&a.last_played.clone().unwrap_or_else(|| a.created.clone()))
        });
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        write_json(&paths.instances_index(), self)
    }

    pub fn get(&self, id: &str) -> Result<&Instance> {
        self.instances
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| LauncherError::NotFound(format!("instance `{id}`")))
    }

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Instance> {
        self.instances
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| LauncherError::NotFound(format!("instance `{id}`")))
    }

    /// Persist a single instance to `<instance>/instance.json` and to the index.
    pub fn persist(&self, paths: &AppPaths, id: &str) -> Result<()> {
        let instance = self.get(id)?;
        write_json(&paths.instance_file(id), instance)?;
        self.save(paths)
    }

    /// Verify that instance directories exist, recreating missing metadata.
    pub fn reconcile(&mut self, paths: &AppPaths) -> Result<()> {
        // Drop index entries whose directory disappeared.
        self.instances.retain(|i| paths.instance_dir(&i.id).exists());

        // Import directories that exist on disk but have no index entry.
        if let Ok(entries) = std::fs::read_dir(paths.instances_dir()) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                // The folder name is the id: every path is derived from it, so
                // a name that doesn't survive a round trip can't be addressed.
                let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if self.instances.iter().any(|i| i.id == id) {
                    continue;
                }
                let meta = paths.instance_file(&id);
                let mut instance = match read_json::<Instance>(&meta) {
                    Ok(Some(instance)) => instance,
                    Ok(None) => continue,
                    // One damaged instance must not keep the others from loading.
                    Err(error) => {
                        log::warn!("instances: skipping {}: {error}", meta.display());
                        continue;
                    }
                };
                instance.prune_invalid_projects();
                // A copied or renamed folder still carries its old id, which
                // points at a folder that isn't this one (or at nothing valid).
                if instance.id != id {
                    instance.id = id;
                    if let Err(error) = write_json(&meta, &instance) {
                        log::warn!("instances: could not update {}: {error}", meta.display());
                    }
                }
                self.instances.push(instance);
            }
        }

        self.sort();
        self.save(paths)?;
        Ok(())
    }
}

/// The game directory for an instance, honouring a custom override.
pub fn game_dir(paths: &AppPaths, instance: &Instance) -> PathBuf {
    instance
        .settings
        .game_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.instance_game_dir(&instance.id))
}

/// Standard content sub-directory for a Modrinth project type.
pub fn content_dir(game_dir: &Path, project_type: &str) -> PathBuf {
    match project_type {
        "resourcepack" => game_dir.join("resourcepacks"),
        "shader" => game_dir.join("shaderpacks"),
        "datapack" => game_dir.join("datapacks"),
        "world" => game_dir.join("saves"),
        _ => game_dir.join("mods"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(project_id: &str, project_type: &str, files: &[&str]) -> InstalledProject {
        InstalledProject {
            project_id: project_id.into(),
            version_id: format!("{project_id}-v"),
            title: format!("title {project_id}"),
            icon_url: None,
            project_type: project_type.into(),
            files: files.iter().map(|file| file.to_string()).collect(),
            installed_at: None,
        }
    }

    fn instance_with(records: Vec<InstalledProject>) -> Instance {
        let mut instance = Instance::new("Pack", "1.21", LoaderKind::Fabric);
        instance.installed_projects = records;
        instance
    }

    /// The frontend mirrors these tags by hand, so the wire shape is a contract.
    /// The quick-play editor used to offer "Multiplayer" against a `multiplayer`
    /// tag, which made every settings save fail to deserialise — pin it down.
    #[test]
    fn quick_play_uses_camel_case_tags() {
        let json = |value: &QuickPlay| serde_json::to_string(value).expect("serialise");

        assert_eq!(json(&QuickPlay::None), r#"{"type":"none"}"#);
        assert_eq!(
            json(&QuickPlay::SingleplayerContinue),
            r#"{"type":"singleplayerContinue"}"#
        );
        assert_eq!(
            json(&QuickPlay::Multiplayer {
                address: "play.example.com".into()
            }),
            r#"{"type":"multiplayer","address":"play.example.com"}"#
        );
        assert_eq!(
            json(&QuickPlay::Singleplayer {
                world: "New World".into()
            }),
            r#"{"type":"singleplayer","world":"New World"}"#
        );

        // And every one of them must parse back: the failing half, and the one
        // the launcher hits again on the next settings save.
        for value in [
            QuickPlay::None,
            QuickPlay::SingleplayerContinue,
            QuickPlay::Multiplayer {
                address: "play.example.com".into(),
            },
            QuickPlay::Singleplayer {
                world: "New World".into(),
            },
        ] {
            let text = json(&value);
            serde_json::from_str::<QuickPlay>(&text).expect("round trip");
        }
    }

    /// The bug this migration exists for: every mod file resolved to one
    /// record named after the pack's free-form `versionId`.
    #[test]
    fn prune_drops_freeform_ids_and_salvages_their_files() {
        let mut instance = instance_with(vec![
            record("26.1.2-1.1", "modpack", &["mods/a.jar", "mods/b.jar"]),
            record("u76hn1pF", "modpack", &[]),
        ]);

        assert!(instance.prune_invalid_projects());
        assert_eq!(instance.installed_projects.len(), 1);

        let pack = &instance.installed_projects[0];
        assert_eq!(pack.project_id, "u76hn1pF");
        assert_eq!(pack.files, vec!["mods/a.jar", "mods/b.jar"]);
    }

    #[test]
    fn prune_keeps_valid_and_untracked_records() {
        let mut instance = instance_with(vec![
            record("u76hn1pF", "modpack", &[]),
            record("P7dR8mSH", "mod", &["mods/api.jar"]),
            // A local `.mrpack` import: no Modrinth id, by design.
            record("", "modpack", &[]),
        ]);

        assert!(!instance.prune_invalid_projects());
        assert_eq!(instance.installed_projects.len(), 3);
    }

    #[test]
    fn prune_without_a_pack_record_just_drops_the_junk() {
        let mut instance = instance_with(vec![record("26.1.2-1.1", "mod", &["mods/a.jar"])]);

        assert!(instance.prune_invalid_projects());
        assert!(instance.installed_projects.is_empty());
    }

    /// A settings save carries a copy loaded when the page opened; it must not
    /// roll back what the launcher recorded since.
    #[test]
    fn user_edits_keep_backend_owned_fields() {
        let mut stored = instance_with(vec![record("P7dR8mSH", "mod", &["mods/api.jar"])]);
        let stale = stored.clone();
        stored.play_time_secs = 3600;
        stored.last_played = Some("2026-11-04T10:00:00Z".into());
        stored.loader_version = Some("0.16.0".into());
        stored.tags.push("orbit-opt-out".into());

        let mut edited = stale;
        edited.name = "Renamed".into();
        edited.group = Some("Survival".into());
        edited.notes = Some("note".into());
        edited.settings.memory_max = 8192;
        edited.installed_projects.clear();
        edited.id = "elsewhere".into();

        let id = stored.id.clone();
        stored.apply_user_edits(edited);
        assert_eq!(stored.name, "Renamed");
        assert_eq!(stored.group.as_deref(), Some("Survival"));
        assert_eq!(stored.notes.as_deref(), Some("note"));
        assert_eq!(stored.settings.memory_max, 8192);
        assert_eq!(stored.id, id);
        assert_eq!(stored.play_time_secs, 3600);
        assert_eq!(stored.last_played.as_deref(), Some("2026-11-04T10:00:00Z"));
        assert_eq!(stored.loader_version.as_deref(), Some("0.16.0"));
        assert_eq!(stored.tags, vec!["orbit-opt-out"]);
        assert_eq!(stored.installed_projects.len(), 1);
    }

    fn scratch_paths(tag: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "orbit-launcher-instances-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("instances")).unwrap();
        AppPaths::new(root)
    }

    /// One broken instance.json or a broken index must not stop startup, and a
    /// folder is always imported under its own name.
    #[test]
    fn reconcile_skips_bad_instances_and_adopts_folder_names() {
        let paths = scratch_paths("reconcile");
        std::fs::write(paths.instances_index(), "{ not json").unwrap();

        let good = Instance::new("Good", "1.21", LoaderKind::Vanilla);
        let mut moved = Instance::new("Moved", "1.21", LoaderKind::Vanilla);
        moved.id = r"..\x".into();
        write_json(&paths.instance_file(&good.id), &good).unwrap();
        write_json(&paths.instance_file("copied-folder"), &moved).unwrap();
        std::fs::create_dir_all(paths.instance_dir("broken")).unwrap();
        std::fs::write(paths.instance_file("broken"), "{ nope").unwrap();

        let mut store = InstanceStore::load(&paths).expect("a broken index still loads");
        assert!(paths.instances_index().with_extension("json.corrupt").is_file());
        store.reconcile(&paths).expect("a broken instance is skipped");

        let mut ids: Vec<&str> = store.instances.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        let mut expected = vec![good.id.as_str(), "copied-folder"];
        expected.sort();
        assert_eq!(ids, expected);
        let on_disk: Instance = read_json(&paths.instance_file("copied-folder")).unwrap().unwrap();
        assert_eq!(on_disk.id, "copied-folder");

        // A second start finds the same instances, not duplicates.
        let mut again = InstanceStore::load(&paths).unwrap();
        again.reconcile(&paths).unwrap();
        assert_eq!(again.instances.len(), 2);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn prune_deduplicates_salvaged_files() {
        let mut instance = instance_with(vec![
            record("26.1.2-1.1", "modpack", &["mods/a.jar"]),
            record("u76hn1pF", "modpack", &["mods/a.jar"]),
        ]);

        assert!(instance.prune_invalid_projects());
        assert_eq!(instance.installed_projects[0].files, vec!["mods/a.jar"]);
    }
}
