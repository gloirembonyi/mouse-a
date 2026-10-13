// Mouse AI Desktop - Git status/diff backend for the IDE dock's Changes tab.
// Shells out to the system `git` binary rather than pulling in git2 - this
// only needs `status --porcelain` + `diff --numstat`/`diff`, which a couple
// of subprocess calls cover without the extra dependency weight.
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

#[tauri::command]
pub fn git_status(repo_path: String) -> Result<Vec<ChangedFile>, String> {
    let status = run_git(&repo_path, &["status", "--porcelain=v1"])?;
    let numstat = run_git(&repo_path, &["diff", "--numstat", "HEAD"]).unwrap_or_default();

    let mut stats: HashMap<String, (u32, u32)> = HashMap::new();
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let add = parts.next().unwrap_or("0");
        let del = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("");
        if !path.is_empty() {
            stats.insert(path.to_string(), (add.parse().unwrap_or(0), del.parse().unwrap_or(0)));
        }
    }

    let mut files = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].trim().to_string();
        let path = line[3..].trim().to_string();
        let (additions, deletions) = stats.get(&path).copied().unwrap_or((0, 0));
        files.push(ChangedFile { path, status: code, additions, deletions });
    }
    Ok(files)
}

#[tauri::command]
pub fn git_diff_file(repo_path: String, file_path: String) -> Result<String, String> {
    // HEAD diff covers modified tracked files; an empty result (new/untracked
    // file) falls back to a plain `diff --no-index` against /dev/null so new
    // files still show their full content as "added" lines.
    let tracked = run_git(&repo_path, &["diff", "HEAD", "--", &file_path]).unwrap_or_default();
    if !tracked.trim().is_empty() {
        return Ok(tracked);
    }
    run_git_allow_failure(&repo_path, &["diff", "--no-index", "--", "/dev/null", &file_path])
}

fn run_git(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let output = spawn_git(repo_path, args).map_err(|e| format!("Failed to run git: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// `git diff --no-index` exits 1 when there IS a difference (which is the
// expected/only case we call it for) - treat that as success and just return
// stdout, only propagating a real error on a spawn failure.
fn run_git_allow_failure(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let output = spawn_git(repo_path, args).map_err(|e| format!("Failed to run git: {}", e))?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn spawn_git(repo_path: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
}
