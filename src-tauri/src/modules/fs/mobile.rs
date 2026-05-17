//! Mobile filesystem command shims.
//!
//! iOS builds should treat rcarmo/ios-linuxkit as the workspace filesystem.
//! These commands preserve the desktop Tauri command names while delegating the
//! operation to the LinuxKit backend protocol.

use serde::{Deserialize, Serialize};

use crate::modules::linuxkit::{FsRequest, LinuxKitBackend, LinuxKitRequest, LinuxKitResponse};
use crate::modules::workspace::WorkspaceEnv;

fn decode_json<T: serde::de::DeserializeOwned>(
    response: LinuxKitResponse,
    command: &str,
) -> Result<T, String> {
    match response {
        LinuxKitResponse::Json { value } => {
            serde_json::from_value(value).map_err(|e| e.to_string())
        }
        _ => Err(format!(
            "ios-linuxkit {command} returned an unexpected response"
        )),
    }
}

fn request_json<T: serde::de::DeserializeOwned>(
    request: FsRequest,
    command: &str,
) -> Result<T, String> {
    let response = LinuxKitBackend::new().request(LinuxKitRequest::Fs(request))?;
    decode_json(response, command)
}

fn request_unit(request: FsRequest) -> Result<(), String> {
    match LinuxKitBackend::new().request(LinuxKitRequest::Fs(request))? {
        LinuxKitResponse::Unit | LinuxKitResponse::Json { .. } => Ok(()),
        _ => Err("ios-linuxkit filesystem command returned an unexpected response".into()),
    }
}

pub mod file {
    use super::*;

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    pub enum ReadResult {
        Text { content: String, size: u64 },
        Binary { size: u64 },
        TooLarge { size: u64, limit: u64 },
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum StatKind {
        File,
        Dir,
        Symlink,
    }

    #[derive(Serialize, Deserialize)]
    pub struct FileStat {
        pub size: u64,
        pub mtime: u64,
        pub kind: StatKind,
    }

    #[tauri::command]
    pub fn fs_read_file(
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<ReadResult, String> {
        let _ = workspace;
        request_json(FsRequest::ReadFile { path }, "fs_read_file")
    }

    #[tauri::command]
    pub fn fs_write_file(
        path: String,
        content: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _ = workspace;
        request_unit(FsRequest::WriteFile { path, content })
    }

    #[tauri::command]
    pub fn fs_canonicalize(
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<String, String> {
        let _ = workspace;
        request_json(FsRequest::Canonicalize { path }, "fs_canonicalize")
    }

    #[tauri::command]
    pub fn fs_stat(path: String, workspace: Option<WorkspaceEnv>) -> Result<FileStat, String> {
        let _ = workspace;
        request_json(FsRequest::Stat { path }, "fs_stat")
    }
}

pub mod tree {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    pub enum EntryKind {
        File,
        Dir,
        Symlink,
    }

    #[derive(Serialize, Deserialize)]
    pub struct DirEntry {
        pub name: String,
        pub kind: EntryKind,
        pub size: u64,
        pub mtime: u64,
    }

    #[tauri::command]
    pub fn fs_read_dir(
        path: String,
        show_hidden: bool,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<Vec<DirEntry>, String> {
        let _ = workspace;
        request_json(FsRequest::ReadDir { path, show_hidden }, "fs_read_dir")
    }

    #[tauri::command]
    pub fn list_subdirs(
        path: String,
        show_hidden: bool,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<Vec<String>, String> {
        let entries = fs_read_dir(path, show_hidden, workspace)?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Dir)
            .map(|entry| entry.name)
            .collect())
    }
}

pub mod mutate {
    use super::*;

    #[tauri::command]
    pub fn fs_create_file(path: String, workspace: Option<WorkspaceEnv>) -> Result<(), String> {
        let _ = workspace;
        request_unit(FsRequest::CreateFile { path })
    }

    #[tauri::command]
    pub fn fs_create_dir(path: String, workspace: Option<WorkspaceEnv>) -> Result<(), String> {
        let _ = workspace;
        request_unit(FsRequest::CreateDir { path })
    }

    #[tauri::command]
    pub fn fs_rename(
        from: String,
        to: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _ = workspace;
        request_unit(FsRequest::Rename { from, to })
    }

    #[tauri::command]
    pub fn fs_delete(path: String, workspace: Option<WorkspaceEnv>) -> Result<(), String> {
        let _ = workspace;
        request_unit(FsRequest::Delete { path })
    }
}

pub mod search {
    use super::*;

    #[derive(Serialize, Deserialize)]
    pub struct SearchHit {
        pub path: String,
        pub rel: String,
        pub name: String,
        pub is_dir: bool,
    }

    #[derive(Serialize, Deserialize)]
    pub struct SearchResult {
        pub hits: Vec<SearchHit>,
        pub truncated: bool,
    }

    #[derive(Serialize, Deserialize)]
    pub struct ListFilesResult {
        pub files: Vec<String>,
        pub truncated: bool,
    }

    #[tauri::command]
    pub fn fs_search(
        root: String,
        query: String,
        limit: Option<usize>,
        workspace: Option<WorkspaceEnv>,
        show_hidden: Option<bool>,
    ) -> Result<SearchResult, String> {
        let _ = (workspace, show_hidden);
        request_json(
            FsRequest::Search {
                root,
                query,
                max_results: limit,
            },
            "fs_search",
        )
    }

    #[tauri::command]
    pub fn fs_list_files(
        root: String,
        limit: Option<usize>,
        max_depth: Option<usize>,
        workspace: Option<WorkspaceEnv>,
        show_hidden: Option<bool>,
    ) -> Result<ListFilesResult, String> {
        let _ = (max_depth, workspace, show_hidden);
        request_json(
            FsRequest::ListFiles {
                root,
                max_results: limit,
            },
            "fs_list_files",
        )
    }
}

pub mod grep {
    use super::*;

    #[derive(Serialize, Deserialize)]
    pub struct GrepHit {
        pub path: String,
        pub rel: String,
        pub line: u64,
        pub text: String,
    }

    #[derive(Serialize, Deserialize)]
    pub struct GrepResponse {
        pub hits: Vec<GrepHit>,
        pub truncated: bool,
        pub files_scanned: usize,
    }

    #[derive(Serialize, Deserialize)]
    pub struct GlobHit {
        pub path: String,
        pub rel: String,
    }

    #[derive(Serialize, Deserialize)]
    pub struct GlobResponse {
        pub hits: Vec<GlobHit>,
        pub truncated: bool,
    }

    #[tauri::command]
    pub fn fs_grep(
        pattern: String,
        root: String,
        glob: Option<Vec<String>>,
        case_insensitive: Option<bool>,
        max_results: Option<usize>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<GrepResponse, String> {
        let _ = workspace;
        request_json(
            FsRequest::Grep {
                root,
                pattern,
                glob,
                case_insensitive,
                max_results,
            },
            "fs_grep",
        )
    }

    #[tauri::command]
    pub fn fs_glob(
        pattern: String,
        root: String,
        max_results: Option<usize>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<GlobResponse, String> {
        let _ = workspace;
        request_json(
            FsRequest::Glob {
                root,
                pattern,
                max_results,
            },
            "fs_glob",
        )
    }
}
