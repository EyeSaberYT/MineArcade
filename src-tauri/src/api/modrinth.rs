//! Minimal, typed client for the Modrinth API (Labrinth) v2.
//!
//! Docs: <https://docs.modrinth.com/api-schema>
//!
//! Only the endpoints the launcher actually needs are modelled. Everything is
//! plain `reqwest` so the same code powers search, project pages, version
//! resolution and update checks.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{LauncherError, Result};

// ---------------------------------------------------------------------------
// Batch limits
// ---------------------------------------------------------------------------

/// Hashes per `/version_files` request. Comfortably under Labrinth's limit
/// and few enough requests that a 300-mod pack stays well inside rate limits.
const HASH_BATCH: usize = 100;

/// Ids per `/projects` request.
const PROJECT_BATCH: usize = 50;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub offset: u32,
    pub limit: u32,
    pub total_hits: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct SearchHit {
    pub project_id: String,
    pub project_type: String,
    pub slug: String,
    pub author: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub display_categories: Vec<String>,
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub date_created: Option<String>,
    #[serde(default)]
    pub date_modified: Option<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub client_side: Option<String>,
    #[serde(default)]
    pub server_side: Option<String>,
    #[serde(default)]
    pub gallery: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Project {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
    pub project_type: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub additional_categories: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub followers: u64,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub gallery: Vec<GalleryItem>,
    #[serde(default)]
    pub license: Option<License>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub issues_url: Option<String>,
    #[serde(default)]
    pub wiki_url: Option<String>,
    #[serde(default)]
    pub discord_url: Option<String>,
    #[serde(default)]
    pub client_side: Option<String>,
    #[serde(default)]
    pub server_side: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct GalleryItem {
    pub url: String,
    #[serde(default)]
    pub featured: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct License {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub files: Vec<VersionFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub date_published: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct VersionFile {
    #[serde(default)]
    pub hashes: HashMap<String, String>,
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub file_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Dependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    /// `required`, `optional`, `incompatible` or `embedded`.
    #[serde(default)]
    pub dependency_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Tag {
    /// Loader and category tags call this `name`; game-version tags call the
    /// same value `version`, so accept either spelling.
    #[serde(alias = "version")]
    pub name: String,
    /// Game-version tags only: `release`, `snapshot`, `alpha` or `beta`.
    #[serde(default)]
    pub version_type: Option<String>,
    #[serde(default)]
    pub project_type: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub header: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A resolved "install this file" instruction produced by the dependency walker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct ResolvedFile {
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    pub project_type: String,
    pub url: String,
    pub filename: String,
    pub sha1: Option<String>,
    pub size: u64,
    pub icon_url: Option<String>,
    /// True when this entry was pulled in as a dependency.
    pub dependency: bool,
}

/// Search filters accepted by the launcher UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "camelCase"))]
pub struct SearchFilters {
    pub query: String,
    /// `mod`, `modpack`, `resourcepack`, `shader`, `datapack`.
    pub project_type: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
    pub categories: Vec<String>,
    /// `relevance`, `downloads`, `follows`, `newest`, `updated`.
    pub index: Option<String>,
}

impl SearchFilters {
    /// Build the Modrinth `facets` query parameter.
    pub fn facets(&self) -> Option<String> {
        let mut groups: Vec<Vec<String>> = vec![];

        if let Some(kind) = &self.project_type {
            groups.push(vec![format!("project_type:{kind}")]);
        }
        if let Some(version) = &self.game_version {
            if !version.is_empty() && version != "any" {
                groups.push(vec![format!("versions:{version}")]);
            }
        }
        if let Some(loader) = &self.loader {
            // Resource packs and shaders are loader-agnostic; datapacks do
            // carry loaders (many require Fabric or Forge).
            let applies = matches!(
                self.project_type.as_deref(),
                Some("mod") | Some("modpack") | Some("datapack") | None
            );
            if applies && !loader.is_empty() && loader != "any" {
                groups.push(vec![format!("categories:{loader}")]);
            }
        }
        if !self.categories.is_empty() {
            groups.push(
                self.categories
                    .iter()
                    .map(|c| format!("categories:{c}"))
                    .collect(),
            );
        }

        if groups.is_empty() {
            None
        } else {
            serde_json::to_string(&groups).ok()
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ModrinthClient {
    http: reqwest::Client,
    base: String,
}

impl ModrinthClient {
    pub fn new(http: reqwest::Client, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let res = self
            .http
            .get(&url)
            .header("User-Agent", crate::USER_AGENT)
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(LauncherError::NotFound(url));
        }
        if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LauncherError::msg(
                "Modrinth is rate limiting us. Try again in a moment.",
            ));
        }
        if !res.status().is_success() {
            return Err(LauncherError::msg(format!(
                "Modrinth returned {} for {path}",
                res.status()
            )));
        }
        Ok(res.json::<T>().await?)
    }

    /// `GET /search`
    pub async fn search(
        &self,
        filters: &SearchFilters,
        limit: u32,
        offset: u32,
    ) -> Result<SearchResponse> {
        let mut url = format!(
            "/search?query={}&limit={}&offset={}",
            urlencode(&filters.query),
            limit.min(100),
            offset
        );
        if let Some(index) = &filters.index {
            url.push_str(&format!("&index={index}"));
        }
        if let Some(facets) = filters.facets() {
            url.push_str(&format!("&facets={}", urlencode(&facets)));
        }
        self.get(&url).await
    }

    /// `GET /project/{id|slug}`
    pub async fn project(&self, id: &str) -> Result<Project> {
        self.get(&format!("/project/{id}")).await
    }

    /// `GET /projects?ids=[…]`
    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let encoded = urlencode(&serde_json::to_string(ids)?);
        self.get(&format!("/projects?ids={encoded}")).await
    }

    /// `GET /project/{id}/version`
    pub async fn versions(
        &self,
        project_id: &str,
        game_versions: &[String],
        loaders: &[String],
    ) -> Result<Vec<Version>> {
        let mut url = format!("/project/{project_id}/version");
        let mut params: Vec<String> = vec![];
        if !game_versions.is_empty() {
            params.push(format!(
                "game_versions={}",
                urlencode(&serde_json::to_string(game_versions)?)
            ));
        }
        if !loaders.is_empty() {
            params.push(format!(
                "loaders={}",
                urlencode(&serde_json::to_string(loaders)?)
            ));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        self.get(&url).await
    }

    /// `GET /version/{id}`
    pub async fn version(&self, id: &str) -> Result<Version> {
        self.get(&format!("/version/{id}")).await
    }

    /// `POST /version_files` – look up versions by file SHA-1.
    pub async fn versions_from_hashes(
        &self,
        hashes: &[String],
    ) -> Result<HashMap<String, Version>> {
        let url = format!("{}/version_files", self.base);
        let res = self
            .http
            .post(&url)
            .header("User-Agent", crate::USER_AGENT)
            .json(&serde_json::json!({
                "hashes": hashes,
                "algorithm": "sha1",
            }))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(LauncherError::msg("Modrinth couldn't match those files."));
        }
        Ok(res.json().await?)
    }

    /// `POST /version_files` for many hashes at once, split into batches.
    ///
    /// Any failed batch fails the whole call: a partial answer would be
    /// indistinguishable from "these files aren't on Modrinth", and callers
    /// need to tell those two situations apart.
    pub async fn versions_from_hashes_batched(
        &self,
        hashes: &[String],
    ) -> Result<HashMap<String, Version>> {
        let mut out = HashMap::new();
        for chunk in hashes.chunks(HASH_BATCH) {
            out.extend(self.versions_from_hashes(chunk).await?);
        }
        Ok(out)
    }

    /// `GET /projects` for many ids at once, split into batches.
    ///
    /// Titles and icons are cosmetic, so a failed batch is skipped rather
    /// than failing the call — callers fall back to what they already know.
    pub async fn projects_batched(&self, ids: &[String]) -> Vec<Project> {
        let mut out = Vec::new();
        for chunk in ids.chunks(PROJECT_BATCH) {
            match self.projects(chunk).await {
                Ok(projects) => out.extend(projects),
                Err(error) => log::debug!("project title lookup failed: {error}"),
            }
        }
        out
    }

    /// `POST /version_files/update` – ask Modrinth for the newest compatible
    /// version of each installed file in one round trip.
    pub async fn updates_from_hashes(
        &self,
        hashes: &[String],
        game_versions: &[String],
        loaders: &[String],
    ) -> Result<HashMap<String, Version>> {
        let url = format!("{}/version_files/update", self.base);
        let res = self
            .http
            .post(&url)
            .header("User-Agent", crate::USER_AGENT)
            .json(&serde_json::json!({
                "hashes": hashes,
                "algorithm": "sha1",
                "game_versions": game_versions,
                "loaders": loaders,
            }))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(LauncherError::msg("Update check failed."));
        }
        Ok(res.json().await?)
    }

    /// `GET /tag/{loader|game_version|category}`
    pub async fn tags(&self, kind: &str) -> Result<Vec<Tag>> {
        self.get(&format!("/tag/{kind}")).await
    }

    // -----------------------------------------------------------------------
    // Dependency resolution
    // -----------------------------------------------------------------------

    /// Walk a version's dependencies and produce the full set of files to
    /// install, mirroring Modrinth's own "Install" behaviour.
    ///
    /// Only `required` dependencies are followed; `optional`/`embedded`/
    /// `incompatible` entries are surfaced to the caller for a decision.
    pub async fn resolve_dependencies(
        &self,
        version: &Version,
        loader: &str,
        game_version: &str,
        depth: usize,
    ) -> Result<Vec<ResolvedFile>> {
        let mut resolved: Vec<ResolvedFile> = vec![];
        let mut seen_projects: Vec<String> = vec![version.project_id.clone()];

        // Breadth-first over required dependencies, capped to avoid cycles.
        let mut queue: Vec<(String, Option<String>)> = version
            .dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
            .map(|d| (d.project_id.clone().unwrap_or_default(), d.version_id.clone()))
            .collect();

        let mut level = 0;
        while !queue.is_empty() && level < depth.max(1) {
            let mut next: Vec<(String, Option<String>)> = vec![];

            for (project_id, version_id) in queue {
                let project = if !project_id.is_empty() {
                    self.project(&project_id).await.ok()
                } else {
                    None
                };

                let Some(project) = project else { continue };
                if seen_projects.contains(&project.id) {
                    continue;
                }
                seen_projects.push(project.id.clone());

                // Find the best version: the pinned one, else the newest that
                // matches the loader + game version.
                let candidate = match version_id.as_deref() {
                    Some(id) => self.version(id).await.ok(),
                    None => self
                        .versions(
                            &project.id,
                            &[game_version.to_string()],
                            &[loader.to_string()],
                        )
                        .await
                        .ok()
                        .and_then(|versions| versions.into_iter().next()),
                };

                let Some(candidate) = candidate else { continue };

                // Queue this dependency's own requirements for the next level.
                for dep in &candidate.dependencies {
                    if dep.dependency_type == "required" {
                        next.push((dep.project_id.clone().unwrap_or_default(), dep.version_id.clone()));
                    }
                }

                if let Some(file) = candidate.files.iter().find(|f| f.primary).or_else(|| candidate.files.first()) {
                    resolved.push(ResolvedFile {
                        project_id: project.id.clone(),
                        version_id: candidate.id.clone(),
                        title: project.title.clone(),
                        project_type: project.project_type.clone(),
                        url: file.url.clone(),
                        filename: file.filename.clone(),
                        sha1: file.hashes.get("sha1").cloned(),
                        size: file.size,
                        icon_url: project.icon_url.clone(),
                        dependency: true,
                    });
                }
            }

            queue = next;
            level += 1;
        }

        Ok(resolved)
    }

    /// Turn the primary file of a version into a [`ResolvedFile`].
    pub async fn primary_file(&self, version: &Version) -> Result<ResolvedFile> {
        let project = self.project(&version.project_id).await.ok();
        let file = version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| version.files.first())
            .ok_or_else(|| LauncherError::msg("This version has no downloadable file."))?;

        Ok(ResolvedFile {
            project_id: version.project_id.clone(),
            version_id: version.id.clone(),
            title: project
                .as_ref()
                .map(|p| p.title.clone())
                .unwrap_or_else(|| version.name.clone()),
            project_type: project
                .as_ref()
                .map(|p| p.project_type.clone())
                .unwrap_or_else(|| "mod".into()),
            url: file.url.clone(),
            filename: file.filename.clone(),
            sha1: file.hashes.get("sha1").cloned(),
            size: file.size,
            icon_url: project.and_then(|p| p.icon_url),
            dependency: false,
        })
    }
}

/// Percent-encode a query-string value (RFC 3986 unreserved characters only).
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed, real-shaped `/search` response. The API is snake_case; this
    /// guards against regressing back to camelCase-only deserialization.
    const SEARCH_JSON: &str = r#"{
        "hits": [{
            "project_id": "AANobbMI",
            "project_type": "mod",
            "slug": "sodium",
            "author": "jellysquid3",
            "title": "Sodium",
            "description": "A high-performance rendering engine replacement.",
            "categories": ["fabric", "optimization"],
            "display_categories": ["fabric", "optimization"],
            "versions": ["1.21.1"],
            "downloads": 245166038,
            "follows": 41770,
            "icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.webp",
            "date_created": "2021-01-03T00:53:34.185936+00:00",
            "date_modified": "2026-09-20T21:27:06.329936+00:00",
            "latest_version": "NM9w06Kr",
            "license": "LicenseRef-Polyform-Shield-1.0.0",
            "client_side": "required",
            "server_side": "unsupported",
            "gallery": []
        }],
        "offset": 0,
        "limit": 1,
        "total_hits": 355
    }"#;

    #[test]
    fn search_response_deserializes_snake_case() {
        let response: SearchResponse = serde_json::from_str(SEARCH_JSON).unwrap();
        assert_eq!(response.total_hits, 355);
        let hit = &response.hits[0];
        assert_eq!(hit.project_id, "AANobbMI");
        assert_eq!(hit.project_type, "mod");
        assert_eq!(hit.display_categories, ["fabric", "optimization"]);
        assert_eq!(hit.latest_version.as_deref(), Some("NM9w06Kr"));
        assert_eq!(hit.client_side.as_deref(), Some("required"));
    }

    #[test]
    fn wire_types_serialize_camel_case_for_the_frontend() {
        let response: SearchResponse = serde_json::from_str(SEARCH_JSON).unwrap();
        let json = serde_json::to_value(&response.hits[0]).unwrap();
        assert_eq!(json["projectId"], "AANobbMI");
        assert!(json.get("project_id").is_none());
        assert!(json.get("displayCategories").is_some());
    }

    /// Decode a facets string back into groups so assertions read as facets.
    fn facet_groups(filters: &SearchFilters) -> Vec<Vec<String>> {
        filters
            .facets()
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap_or_default()
    }

    #[test]
    fn facets_group_filters_with_and() {
        let filters = SearchFilters {
            query: "sodium".into(),
            project_type: Some("mod".into()),
            game_version: Some("1.21.1".into()),
            loader: Some("fabric".into()),
            ..Default::default()
        };
        assert_eq!(
            facet_groups(&filters),
            vec![
                vec!["project_type:mod".to_string()],
                vec!["versions:1.21.1".to_string()],
                vec!["categories:fabric".to_string()],
            ]
        );
    }

    /// Resource packs and shaders have no loaders, so the facet must be
    /// dropped for them rather than filtering every project out. Datapacks
    /// do carry loaders.
    #[test]
    fn loader_facet_only_applies_where_loaders_exist() {
        let loader_facet = |project_type: &str| {
            let filters = SearchFilters {
                project_type: Some(project_type.into()),
                loader: Some("fabric".into()),
                ..Default::default()
            };
            facet_groups(&filters)
        };

        assert!(loader_facet("mod").contains(&vec!["categories:fabric".to_string()]));
        assert!(loader_facet("datapack").contains(&vec!["categories:fabric".to_string()]));
        assert!(!loader_facet("resourcepack").iter().any(|g| g[0].starts_with("categories:")));
        assert!(!loader_facet("shader").iter().any(|g| g[0].starts_with("categories:")));
    }

    /// "Any loader" placeholders must not turn into a facet that matches
    /// nothing.
    #[test]
    fn facets_ignore_empty_and_any_placeholders() {
        for value in ["", "any"] {
            let filters = SearchFilters {
                project_type: Some("mod".into()),
                game_version: Some(value.into()),
                loader: Some(value.into()),
                ..Default::default()
            };
            assert_eq!(facet_groups(&filters), vec![vec!["project_type:mod".to_string()]]);
        }
    }

    #[test]
    fn version_deserializes_snake_case() {
        let json = r#"{
            "id": "NM9w06Kr",
            "project_id": "AANobbMI",
            "name": "Sodium 0.6.0",
            "version_number": "mc1.21.1-0.6.0",
            "version_type": "release",
            "game_versions": ["1.21.1"],
            "loaders": ["fabric"],
            "downloads": 1234,
            "date_published": "2024-08-01T00:00:00Z",
            "changelog": "fixes",
            "dependencies": [{
                "version_id": null,
                "project_id": "P7dR8mSH",
                "file_name": null,
                "dependency_type": "required"
            }],
            "files": [{
                "hashes": { "sha1": "abc", "sha512": "def" },
                "url": "https://cdn.modrinth.com/data/AANobbMI/version.jar",
                "filename": "sodium.jar",
                "primary": true,
                "size": 42,
                "file_type": null
            }]
        }"#;
        let version: Version = serde_json::from_str(json).unwrap();
        assert_eq!(version.project_id, "AANobbMI");
        assert_eq!(version.version_number, "mc1.21.1-0.6.0");
        assert_eq!(version.files[0].hashes["sha1"], "abc");
        assert_eq!(version.dependencies[0].dependency_type, "required");
        assert_eq!(
            version.dependencies[0].project_id.as_deref(),
            Some("P7dR8mSH")
        );
    }

    /// `/tag/game_version` calls the value `version` and carries no `name`, so
    /// requiring `name` made this endpoint fail to deserialize outright.
    #[test]
    fn game_version_tags_deserialize_from_the_version_key() {
        let json = r#"[
            {
                "version": "26.3",
                "version_type": "release",
                "date": "2026-10-13T12:05:46Z",
                "major": true
            },
            {
                "version": "26.4-snapshot-4",
                "version_type": "snapshot",
                "date": "2026-10-13T12:05:46Z",
                "major": false
            }
        ]"#;
        let tags: Vec<Tag> = serde_json::from_str(json).unwrap();
        assert_eq!(tags[0].name, "26.3");
        assert_eq!(tags[0].version_type.as_deref(), Some("release"));
        assert_eq!(tags[1].name, "26.4-snapshot-4");
        assert_eq!(tags[1].version_type.as_deref(), Some("snapshot"));
    }

    /// The loader and category endpoints keep using `name`, and version_type is
    /// absent there, so it must stay optional.
    #[test]
    fn loader_tags_still_deserialize_from_the_name_key() {
        let json = r#"[{"name": "fabric", "icon": "<svg/>"}]"#;
        let tags: Vec<Tag> = serde_json::from_str(json).unwrap();
        assert_eq!(tags[0].name, "fabric");
        assert_eq!(tags[0].version_type, None);
        assert_eq!(tags[0].icon.as_deref(), Some("<svg/>"));
    }

    #[test]
    fn tags_serialize_version_type_as_camel_case() {
        let tags: Vec<Tag> = serde_json::from_str(
            r#"[{"version": "26.3", "version_type": "release"}]"#,
        )
        .unwrap();
        let json = serde_json::to_value(&tags[0]).unwrap();
        assert_eq!(json["name"], "26.3");
        assert_eq!(json["versionType"], "release");
        assert!(json.get("version_type").is_none());
    }
}
