use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use fs::Fs;
use serde::{Deserialize, Serialize};

pub const PROJECT_FILE_EXTENSION: &str = "zed-project";

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    #[serde(default)]
    pub folders: Vec<ProjectFileFolder>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectFileFolder {
    pub path: String,
}

pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PROJECT_FILE_EXTENSION))
}

impl ProjectFile {
    pub fn parse(contents: &str) -> Result<Self> {
        serde_json::from_str(contents).context("invalid project file")
    }

    /// Folders that live under the project file's directory are stored as
    /// relative paths so the project file keeps working after the whole tree
    /// is moved or checked out elsewhere.
    pub fn from_folders(folders: &[PathBuf], project_file_directory: &Path) -> Self {
        let folders = folders
            .iter()
            .map(|folder| {
                let path = match folder.strip_prefix(project_file_directory) {
                    Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
                    Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
                    Err(_) => folder.to_string_lossy().into_owned(),
                };
                ProjectFileFolder { path }
            })
            .collect();
        Self { folders }
    }

    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn resolve_folders(&self, project_file_directory: &Path) -> Vec<PathBuf> {
        self.folders
            .iter()
            .map(|folder| {
                let path = Path::new(&folder.path);
                let joined = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    project_file_directory.join(path)
                };
                util::paths::normalize_lexically(&joined).unwrap_or(joined)
            })
            .collect()
    }
}

/// Replaces every `.zed-project` file in `paths` with the folders it lists,
/// keeping all other paths untouched and in order.
pub async fn expand_project_files(paths: Vec<PathBuf>, fs: &dyn Fs) -> Result<Vec<PathBuf>> {
    if !paths.iter().any(|path| is_project_file(path)) {
        return Ok(paths);
    }

    let mut expanded = Vec::with_capacity(paths.len());
    for path in paths {
        if !is_project_file(&path) || !fs.is_file(&path).await {
            expanded.push(path);
            continue;
        }
        let contents = fs
            .load(&path)
            .await
            .with_context(|| format!("failed to read project file {}", path.display()))?;
        let project_file = ProjectFile::parse(&contents)
            .with_context(|| format!("failed to parse project file {}", path.display()))?;
        let project_file_directory = path.parent().unwrap_or(Path::new(""));
        for folder in project_file.resolve_folders(project_file_directory) {
            if !expanded.contains(&folder) {
                expanded.push(folder);
            }
        }
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip_relative_and_absolute_folders() {
        let base = if cfg!(windows) {
            PathBuf::from("C:\\work\\app")
        } else {
            PathBuf::from("/work/app")
        };
        let outside = if cfg!(windows) {
            PathBuf::from("D:\\shared\\lib")
        } else {
            PathBuf::from("/shared/lib")
        };
        let folders = vec![base.clone(), base.join("crates").join("core"), outside.clone()];

        let project_file = ProjectFile::from_folders(&folders, &base);
        assert_eq!(project_file.folders[0].path, ".");
        assert_eq!(project_file.folders[1].path, "crates/core");

        let json = project_file.to_json().expect("serializes");
        let parsed = ProjectFile::parse(&json).expect("parses");
        assert_eq!(parsed.resolve_folders(&base), folders);
    }

    #[test]
    fn test_parent_relative_folder() {
        let base = if cfg!(windows) {
            PathBuf::from("C:\\work\\app")
        } else {
            PathBuf::from("/work/app")
        };
        let parsed = ProjectFile::parse(r#"{"folders":[{"path":"../lib"}]}"#).expect("parses");
        assert_eq!(
            parsed.resolve_folders(&base),
            vec![base.parent().expect("has parent").join("lib")]
        );
    }
}
