//! CurseForge API client and normalization into Modrinth-shaped types.

use std::collections::HashMap;

use serde::Deserialize;

use crate::api::modrinth::{self, GalleryItem, License, ResolvedFile};
use crate::error::{LauncherError, Result};

pub const CURSEFORGE_API_KEY: &str = "$2a$10$e5XXQKI74zV.SHywaz7l2.9ELP/Bd9lYydtqrTBjNtuFFO8owmDTW";

/// Minecraft's id in CurseForge's game catalogue. `/v1/mods/search` answers
/// `400 Bad Request` unless it is sent, so every search carries it.
const MINECRAFT_GAME_ID: i32 = 432;
const API_BASE: &str = "https://api.curseforge.com";

fn cf_id_to_modrinth(id: i64) -> String {
    format!("cf:{id}")
}

/// Extract the numeric CurseForge project id from a namespaced id
/// (`cf:123` → `123`); `None` for anything else.
pub fn modrinth_id_to_cf(id: &str) -> Option<i64> {
    if let Some(rest) = id.strip_prefix("cf:") {
        rest.parse::<i64>().ok()
    } else {
        None
    }
}

fn version_to_modrinth(project_id: &str, file_id: i64) -> String {
    if let Some(cf) = project_id.strip_prefix("cf:") {
        format!("cf:{cf}:{file_id}")
    } else {
        format!("cf:{project_id}:{file_id}")
    }
}

fn modrinth_version_to_cf(version_id: &str) -> Option<(i64, i64)> {
    if let Some(rest) = version_id.strip_prefix("cf:") {
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let (Ok(mod_id), Ok(file_id)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
                return Some((mod_id, file_id));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfPagination {
    index: u32,
    page_size: u32,
    total_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMod {
    id: i64,
    name: String,
    slug: String,
    summary: Option<String>,
    #[serde(default)]
    categories: Vec<CfCategory>,
    download_count: i64,
    date_modified: Option<String>,
    date_created: Option<String>,
    date_published: Option<String>,
    logo: Option<CfAsset>,
    screenshots: Option<Vec<CfAsset>>,
    links: Option<CfLinks>,
    authors: Option<Vec<CfAuthor>>,
    game_versions: Option<Vec<String>>,
    license: Option<CfLicense>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfCategory {
    name: String,
    slug: String,
    class_id: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfAsset {
    title: Option<String>,
    description: Option<String>,
    thumbnail_url: Option<String>,
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLinks {
    source_url: Option<String>,
    issues_url: Option<String>,
    wiki_url: Option<String>,
    discord_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfAuthor {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLicense {
    id: Option<String>,
    name: String,
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfSearchResponse {
    pagination: Option<CfPagination>,
    data: Vec<CfMod>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfModResponse {
    data: CfMod,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDescriptionResponse {
    data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    id: i64,
    mod_id: i64,
    file_name: String,
    display_name: Option<String>,
    file_length: i64,
    download_url: Option<String>,
    hashes: Vec<CfHash>,
    game_versions: Vec<String>,
    dependencies: Vec<CfDependency>,
    release_type: i32,
    is_available: bool,
    file_date: Option<String>,
    changelog: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfHash {
    value: String,
    algo: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDependency {
    mod_id: i64,
    relation_type: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFileResponse {
    data: CfFile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFilesResponse {
    data: Vec<CfFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDownloadUrlResponse {
    data: String,
}

#[derive(Debug, Clone)]
pub struct CurseForgeClient {
    http: reqwest::Client,
}

impl CurseForgeClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{API_BASE}{path}")
        };
        let res = self
            .http
            .get(&url)
            .header("x-api-key", CURSEFORGE_API_KEY)
            .header("User-Agent", crate::USER_AGENT)
            .send()
            .await?;
        if res.status().is_success() {
            Ok(res.json::<T>().await?)
        } else {
            let status = res.status();
            // CurseForge explains refusals in the body ("gameId is required"),
            // which is far more useful than the status code alone.
            let detail = res.text().await.unwrap_or_default();
            let detail = detail.trim();
            let suffix = if detail.is_empty() {
                String::new()
            } else {
                format!(" — {}", detail.chars().take(200).collect::<String>())
            };
            Err(LauncherError::msg(format!(
                "CurseForge returned {status} for {path}{suffix}"
            )))
        }
    }

    pub async fn search(
        &self,
        filters: &modrinth::SearchFilters,
        limit: u32,
        offset: u32,
    ) -> Result<modrinth::SearchResponse> {
        let path = format!("/v1/mods/search?{}", search_query(filters, limit, offset));
        let resp: CfSearchResponse = self.get(&path).await?;
        let total = resp
            .pagination
            .as_ref()
            .map(|p| p.total_count)
            .unwrap_or_else(|| resp.data.len() as u32);
        let offset_out = resp.pagination.as_ref().map(|p| p.index).unwrap_or(offset);
        let limit_out = resp
            .pagination
            .as_ref()
            .map(|p| p.page_size)
            .unwrap_or(limit);
        Ok(modrinth::SearchResponse {
            hits: resp.data.into_iter().map(cf_mod_to_hit).collect(),
            offset: offset_out,
            limit: limit_out,
            total_hits: total,
        })
    }

    pub async fn project(&self, id: &str) -> Result<modrinth::Project> {
        let Some(cid) = modrinth_id_to_cf(id) else {
            return Err(LauncherError::msg("invalid curseforge id"));
        };
        let resp: CfModResponse = self.get(&format!("/v1/mods/{cid}")).await?;
        let body = self.mod_description(cid).await.unwrap_or_default();
        Ok(cf_mod_to_project(&resp.data, body))
    }

    async fn mod_description(&self, id: i64) -> Result<String> {
        let resp: CfDescriptionResponse = self.get(&format!("/v1/mods/{id}/description")).await?;
        Ok(resp.data)
    }

    pub async fn versions(
        &self,
        project_id: &str,
        game_versions: &[String],
        loaders: &[String],
    ) -> Result<Vec<modrinth::Version>> {
        let Some(cid) = modrinth_id_to_cf(project_id) else {
            return Err(LauncherError::msg("invalid curseforge id"));
        };
        let mut params: Vec<String> = vec![];
        if !game_versions.is_empty() && !game_versions[0].is_empty() {
            params.push(format!(
                "gameVersion={}",
                modrinth::urlencode(&game_versions[0])
            ));
        }
        if !loaders.is_empty() && !loaders[0].is_empty() {
            let m = cf_modloader_name(&loaders[0]);
            if let Some(m) = m {
                params.push(format!("modLoaderType={m}"));
            }
        }
        params.push("sortOrder=desc".to_string());
        params.push("pageSize=50".to_string());
        let path = format!("/v1/mods/{cid}/files?{}", params.join("&"));
        let resp: CfFilesResponse = self.get(&path).await?;
        Ok(resp
            .data
            .into_iter()
            .filter(|f| f.is_available)
            .map(|f| cf_file_to_version(&f, project_id))
            .collect())
    }

    pub async fn version(&self, version_id: &str) -> Result<modrinth::Version> {
        let Some((mod_id, file_id)) = modrinth_version_to_cf(version_id) else {
            return Err(LauncherError::msg("invalid version id"));
        };
        let file = self.file(mod_id, file_id).await?;
        Ok(cf_file_to_version(&file, &cf_id_to_modrinth(mod_id)))
    }

    /// `GET /v1/mods/{modId}/files/{fileId}` — one file in the normalised
    /// [`modrinth::Version`] shape.
    async fn file(&self, mod_id: i64, file_id: i64) -> Result<CfFile> {
        let resp: CfFileResponse = self
            .get(&format!("/v1/mods/{mod_id}/files/{file_id}"))
            .await?;
        Ok(resp.data)
    }

    /// `POST /v1/mods/files` — resolve many files at once (used by the CurseForge
    /// modpack manifest installer, which ships hundreds of project/file id pairs).
    pub async fn files_batch(&self, file_ids: &[i64]) -> Result<Vec<modrinth::Version>> {
        if file_ids.is_empty() {
            return Ok(vec![]);
        }
        let url = format!("{API_BASE}/v1/mods/files");
        let res = self
            .http
            .post(&url)
            .header("x-api-key", CURSEFORGE_API_KEY)
            .header("User-Agent", crate::USER_AGENT)
            .json(&serde_json::json!({ "fileIds": file_ids }))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(LauncherError::msg(format!(
                "CurseForge returned {} for /v1/mods/files",
                res.status()
            )));
        }
        let resp: CfFilesResponse = res.json().await?;
        Ok(resp
            .data
            .into_iter()
            .map(|f| cf_file_to_version(&f, &cf_id_to_modrinth(f.mod_id)))
            .collect())
    }

    /// `POST /v1/mods` — fetch many projects at once (modpack installs use this
    /// to resolve titles and icons without one request per mod).
    pub async fn mods_batch(&self, mod_ids: &[i64]) -> Result<Vec<modrinth::Project>> {
        if mod_ids.is_empty() {
            return Ok(vec![]);
        }
        let url = format!("{API_BASE}/v1/mods");
        let res = self
            .http
            .post(&url)
            .header("x-api-key", CURSEFORGE_API_KEY)
            .header("User-Agent", crate::USER_AGENT)
            .json(&serde_json::json!({ "modIds": mod_ids }))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(LauncherError::msg(format!(
                "CurseForge returned {} for /v1/mods",
                res.status()
            )));
        }
        let resp: CfSearchResponse = res.json().await?;
        Ok(resp
            .data
            .into_iter()
            .map(|m| cf_mod_to_project(&m, String::new()))
            .collect())
    }

    /// `GET /v1/mods/{modId}/files/{fileId}/download-url` — fallback for files
    /// whose inline `downloadUrl` is null.
    pub async fn download_url(&self, mod_id: i64, file_id: i64) -> Result<String> {
        let resp: CfDownloadUrlResponse = self
            .get(&format!("/v1/mods/{mod_id}/files/{file_id}/download-url"))
            .await?;
        Ok(resp.data)
    }

    pub async fn primary_file(&self, version: &modrinth::Version) -> Result<ResolvedFile> {
        let project = self.project(&version.project_id).await.ok();
        let file = version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| version.files.first())
            .ok_or_else(|| LauncherError::msg("This version has no downloadable file."))?;

        // Files published without an inline URL need the download-url endpoint.
        let mut url = file.url.clone();
        if url.is_empty() {
            let Some((mod_id, file_id)) = modrinth_version_to_cf(&version.id) else {
                return Err(LauncherError::msg("invalid version id"));
            };
            url = self.download_url(mod_id, file_id).await?;
        }

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
            url,
            filename: file.filename.clone(),
            sha1: file.hashes.get("sha1").cloned(),
            size: file.size,
            icon_url: project.and_then(|p| p.icon_url),
            dependency: false,
        })
    }

    /// Walk a version's required dependencies and produce the full set of
    /// files to install — the CurseForge counterpart of
    /// [`ModrinthClient::resolve_dependencies`](super::modrinth::ModrinthClient::resolve_dependencies).
    ///
    /// CurseForge dependency entries never pin a specific file, so each
    /// dependency resolves to the newest file matching the instance's game
    /// version and loader.
    pub async fn resolve_dependencies(
        &self,
        version: &modrinth::Version,
        loader: &str,
        game_version: &str,
        depth: usize,
    ) -> Result<Vec<ResolvedFile>> {
        let mut resolved: Vec<ResolvedFile> = vec![];
        let mut seen_projects: Vec<String> = vec![version.project_id.clone()];

        // Breadth-first over required dependencies, capped to avoid cycles.
        let mut queue: Vec<String> = version
            .dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
            .filter_map(|d| d.project_id.clone())
            .filter(|id| !id.is_empty())
            .collect();

        let mut level = 0;
        while !queue.is_empty() && level < depth.max(1) {
            let mut next: Vec<String> = vec![];

            for project_id in queue {
                let Ok(project) = self.project(&project_id).await else {
                    continue;
                };
                if seen_projects.contains(&project.id) {
                    continue;
                }
                seen_projects.push(project.id.clone());

                let Some(candidate) = self
                    .versions(
                        &project.id,
                        &[game_version.to_string()],
                        &[loader.to_string()],
                    )
                    .await
                    .ok()
                    .and_then(|versions| versions.into_iter().next())
                else {
                    continue;
                };

                for dep in &candidate.dependencies {
                    if dep.dependency_type == "required" {
                        if let Some(id) = &dep.project_id {
                            next.push(id.clone());
                        }
                    }
                }

                if let Ok(file) = self.primary_file(&candidate).await {
                    resolved.push(ResolvedFile {
                        dependency: true,
                        ..file
                    });
                }
            }

            queue = next;
            level += 1;
        }

        Ok(resolved)
    }
}

/// Build the `/v1/mods/search` query string.
///
/// `gameId` is mandatory — the API answers `400 Bad Request` without it — so
/// every search is scoped to Minecraft; `classId` then narrows it to mods,
/// modpacks, resource packs, shaders or datapacks.
fn search_query(filters: &modrinth::SearchFilters, limit: u32, offset: u32) -> String {
    let mut params: Vec<String> = vec![format!("gameId={MINECRAFT_GAME_ID}")];
    if !filters.query.is_empty() {
        params.push(format!(
            "searchFilter={}",
            modrinth::urlencode(&filters.query)
        ));
    }
    if let Some(cid) = class_id_for_type(&filters.project_type) {
        params.push(format!("classId={cid}"));
    }
    if let Some(game) = &filters.game_version {
        if !game.is_empty() && game != "any" {
            params.push(format!("gameVersion={}", modrinth::urlencode(game)));
        }
    }
    if let Some(m) = cf_modloader(&filters.loader) {
        params.push(format!("modLoaderType={m}"));
    }
    // CurseForge's sortField is numeric (2 = popularity, 3 = last updated,
    // 6 = name). Omitting it entirely gives relevance ranking, which is what
    // we want when the user typed a query.
    let sort_field = match filters.index.as_deref().unwrap_or("relevance") {
        "downloads" | "follows" | "popularity" => Some(2),
        "updated" | "newest" => Some(3),
        "name" => Some(6),
        _ => None,
    };
    if let Some(sf) = sort_field {
        params.push(format!("sortField={sf}"));
    }
    // sortOrder must be sent explicitly: without it the API silently falls
    // back to alphabetical ordering regardless of sortField.
    params.push("sortOrder=desc".to_string());
    params.push(format!("index={offset}"));
    params.push(format!("pageSize={}", limit.min(50)));
    params.join("&")
}

fn cf_modloader(loader: &Option<String>) -> Option<i32> {
    match loader.as_deref() {
        Some("fabric") => Some(4),
        Some("forge") => Some(1),
        Some("quilt") => Some(5),
        Some("neoforge") => Some(6),
        _ => None,
    }
}

fn cf_modloader_name(loader: &str) -> Option<i32> {
    cf_modloader(&Some(loader.to_string()))
}

fn cf_mod_to_hit(m: CfMod) -> modrinth::SearchHit {
    let mut gallery: Vec<String> = vec![];
    if let Some(screens) = &m.screenshots {
        for s in screens {
            if let Some(th) = &s.thumbnail_url {
                gallery.push(th.clone());
            } else {
                gallery.push(s.url.clone());
            }
        }
    }
    modrinth::SearchHit {
        project_id: cf_id_to_modrinth(m.id),
        project_type: class_to_project_type(
            m.categories.first().and_then(|c| c.class_id).unwrap_or(6),
        ),
        slug: m.slug.clone(),
        author: m
            .authors
            .as_ref()
            .and_then(|a| a.first())
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        title: m.name.clone(),
        description: m.summary.clone().unwrap_or_default(),
        categories: m.categories.iter().map(|c| c.slug.clone()).collect(),
        display_categories: m.categories.iter().map(|c| c.name.clone()).collect(),
        versions: m.game_versions.clone().unwrap_or_default(),
        downloads: m.download_count as u64,
        follows: 0,
        icon_url: m.logo.as_ref().map(|l| l.url.clone()),
        date_created: m.date_created.clone(),
        date_modified: m.date_modified.clone(),
        latest_version: None,
        license: m.license.as_ref().map(|l| l.name.clone()),
        client_side: None,
        server_side: None,
        gallery,
    }
}

fn cf_mod_to_project(m: &CfMod, body: String) -> modrinth::Project {
    modrinth::Project {
        id: cf_id_to_modrinth(m.id),
        slug: m.slug.clone(),
        title: m.name.clone(),
        description: m.summary.clone().unwrap_or_default(),
        body,
        project_type: class_to_project_type(
            m.categories.first().and_then(|c| c.class_id).unwrap_or(6),
        ),
        categories: m.categories.iter().map(|c| c.slug.clone()).collect(),
        additional_categories: vec![],
        game_versions: m.game_versions.clone().unwrap_or_default(),
        loaders: vec![],
        downloads: m.download_count as u64,
        followers: 0,
        icon_url: m.logo.as_ref().map(|l| l.url.clone()),
        gallery: m
            .screenshots
            .as_ref()
            .unwrap_or(&vec![])
            .iter()
            .map(|s| GalleryItem {
                url: s.url.clone(),
                featured: false,
                title: s.title.clone(),
                description: s.description.clone(),
            })
            .collect(),
        license: m.license.as_ref().map(|l| License {
            id: l.id.clone().unwrap_or_default(),
            name: l.name.clone(),
            url: l.url.clone(),
        }),
        source_url: m.links.as_ref().and_then(|ln| ln.source_url.clone()),
        issues_url: m.links.as_ref().and_then(|ln| ln.issues_url.clone()),
        wiki_url: m.links.as_ref().and_then(|ln| ln.wiki_url.clone()),
        discord_url: m.links.as_ref().and_then(|ln| ln.discord_url.clone()),
        client_side: None,
        server_side: None,
        team: None,
        published: m.date_published.clone(),
        updated: m.date_modified.clone(),
        versions: vec![],
    }
}

fn class_to_project_type(class_id: i32) -> String {
    match class_id {
        4471 => "modpack".to_string(),
        12 => "resourcepack".to_string(),
        6552 => "shader".to_string(),
        6945 => "datapack".to_string(),
        _ => "mod".to_string(),
    }
}

fn class_id_for_type(ty: &Option<String>) -> Option<i32> {
    match ty.as_deref() {
        Some("mod") => Some(6),
        Some("modpack") => Some(4471),
        Some("resourcepack") => Some(12),
        Some("shader") => Some(6552),
        Some("datapack") => Some(6945),
        _ => None,
    }
}

fn cf_file_to_version(f: &CfFile, project_id: &str) -> modrinth::Version {
    let mut loaders: Vec<String> = vec![];
    for v in &f.game_versions {
        let low = v.to_ascii_lowercase();
        if low == "fabric" {
            loaders.push("fabric".to_string());
        }
        if low == "forge" {
            loaders.push("forge".to_string());
        }
        if low == "quilt" {
            loaders.push("quilt".to_string());
        }
        if low == "neoforge" {
            loaders.push("neoforge".to_string());
        }
    }
    let mut game_versions: Vec<String> = vec![];
    for v in &f.game_versions {
        let low = v.to_ascii_lowercase();
        if low == "fabric"
            || low == "forge"
            || low == "quilt"
            || low == "neoforge"
            || low == "client"
            || low == "server"
        {
            continue;
        }
        game_versions.push(v.clone());
    }
    let mut hashes = HashMap::new();
    for h in &f.hashes {
        if h.algo == 1 {
            hashes.insert("sha1".to_string(), h.value.clone());
        }
    }
    modrinth::Version {
        id: version_to_modrinth(project_id, f.id),
        project_id: project_id.to_string(),
        name: f
            .display_name
            .clone()
            .unwrap_or_else(|| f.file_name.clone()),
        version_number: f.file_name.clone(),
        version_type: match f.release_type {
            1 => "release".to_string(),
            2 => "beta".to_string(),
            3 => "alpha".to_string(),
            _ => "release".to_string(),
        },
        game_versions,
        loaders,
        files: vec![modrinth::VersionFile {
            hashes,
            url: f.download_url.clone().unwrap_or_default(),
            filename: f.file_name.clone(),
            primary: true,
            size: f.file_length as u64,
            file_type: None,
        }],
        dependencies: f
            .dependencies
            .iter()
            .filter(|d| d.relation_type == 3)
            .map(|d| modrinth::Dependency {
                version_id: None,
                project_id: Some(cf_id_to_modrinth(d.mod_id)),
                file_name: None,
                dependency_type: "required".to_string(),
            })
            .collect(),
        downloads: 0,
        date_published: f.file_date.clone(),
        changelog: f.changelog.clone(),
    }
}

/// Extra HTTP headers required to fetch a CurseForge-hosted file.
///
/// The file CDN has started requiring the API key (announced July 2026);
/// sending it everywhere on those hosts is harmless where enforcement isn't
/// live yet and future-proof where it is.
pub fn download_headers(url: &str) -> Vec<(String, String)> {
    if let Ok(parsed) = ::url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("");
        if parsed.scheme() == "https" && (host == "edge.forgecdn.net" || host.ends_with(".forgecdn.net")) {
            let key = std::env::var("MINEARCADE_CURSEFORGE_API_KEY").unwrap_or_else(|_| CURSEFORGE_API_KEY.to_string());
            if !key.is_empty() {
                return vec![("x-api-key".to_string(), key)];
            }
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        assert_eq!(cf_id_to_modrinth(123), "cf:123");
        assert_eq!(modrinth_id_to_cf("cf:123"), Some(123));
        assert_eq!(modrinth_id_to_cf("modrinth123"), None);
    }

    #[test]
    fn version_roundtrip() {
        assert_eq!(version_to_modrinth("cf:123", 456), "cf:123:456");
        assert_eq!(modrinth_version_to_cf("cf:123:456"), Some((123, 456)));
    }

    #[test]
    fn loader_names_map_to_cf_enum() {
        assert_eq!(cf_modloader(&Some("fabric".into())), Some(4));
        assert_eq!(cf_modloader(&Some("forge".into())), Some(1));
        assert_eq!(cf_modloader(&Some("quilt".into())), Some(5));
        assert_eq!(cf_modloader(&Some("neoforge".into())), Some(6));
        assert_eq!(cf_modloader(&None), None);
        assert_eq!(cf_modloader(&Some("vanilla".into())), None);
    }

    #[test]
    fn search_query_always_scopes_to_minecraft() {
        let filters = modrinth::SearchFilters {
            query: "sodium".into(),
            project_type: Some("mod".into()),
            game_version: Some("1.21.1".into()),
            loader: Some("fabric".into()),
            index: Some("downloads".into()),
            ..Default::default()
        };

        let query = search_query(&filters, 20, 0);

        assert!(
            query.starts_with("gameId=432&"),
            "gameId is required by the API: {query}"
        );
        assert!(query.contains("searchFilter=sodium"));
        assert!(query.contains("classId=6"));
        assert!(query.contains("gameVersion=1.21.1"));
        assert!(query.contains("modLoaderType=4"));
        assert!(query.contains("sortField=2"));
        assert!(query.contains("sortOrder=desc"));
        assert!(query.contains("index=0"));
        assert!(query.contains("pageSize=20"));
    }

    #[test]
    fn search_query_omits_sort_field_for_relevance() {
        let filters = modrinth::SearchFilters {
            project_type: Some("modpack".into()),
            ..Default::default()
        };

        // 100 requested: the endpoint caps a page at 50.
        let query = search_query(&filters, 100, 40);

        assert!(query.starts_with("gameId=432"));
        assert!(query.contains("classId=4471"));
        assert!(!query.contains("searchFilter"));
        assert!(!query.contains("sortField"));
        assert!(query.contains("sortOrder=desc"));
        assert!(query.contains("index=40"));
        assert!(query.contains("pageSize=50"));
    }

    #[test]
    fn cdn_downloads_carry_the_api_key() {
        let url = "https://edge.forgecdn.net/files/8893/488/geckolib.jar";
        let headers = download_headers(url);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "x-api-key");

        assert!(download_headers("https://cdn.modrinth.com/data/x.jar").is_empty());
    }
}
