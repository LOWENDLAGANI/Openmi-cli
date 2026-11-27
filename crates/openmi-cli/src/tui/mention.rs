use std::path::Path;

use ignore::WalkBuilder;

/// Extract the active `@query` token immediately preceding `cursor_byte`, returning
/// `(at_start_byte, query_str)`.
pub fn active_mention_query(input: &str, cursor_byte: usize) -> Option<(usize, &str)> {
    let prefix = input.get(..cursor_byte)?;
    let token_start = prefix
        .rfind(|c: char| c.is_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    let token = &prefix[token_start..];
    let query = token.strip_prefix('@')?;
    Some((token_start, query))
}

/// Walk workspace files respecting `.gitignore` (via `ignore` crate) and return sorted relative paths.
pub fn collect_workspace_files(root: &Path, max_files: usize) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                let s = rel.to_string_lossy().replace('\\', "/");
                files.push(s);
                if files.len() >= max_files {
                    break;
                }
            }
        }
    }
    files.sort();
    files
}

/// Fuzzy subsequence match returning top `limit` candidate paths for `query`.
pub fn fuzzy_match_paths(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let q_lower = query.to_ascii_lowercase();
    if q_lower.is_empty() {
        return files.iter().take(limit).cloned().collect();
    }

    let mut scored: Vec<(i64, &String)> = files
        .iter()
        .filter_map(|path| fuzzy_score(path, &q_lower).map(|score| (score, path)))
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, p)| p.clone())
        .collect()
}

fn fuzzy_score(candidate: &str, query_lower: &str) -> Option<i64> {
    let cand_lower = candidate.to_ascii_lowercase();
    let mut score: i64 = 0;
    let mut last_match: Option<usize> = None;
    let mut search_from = 0usize;

    for qc in query_lower.chars() {
        let rest = &cand_lower[search_from..];
        let rel_idx = rest.find(qc)?;
        let abs_idx = search_from + rel_idx;

        score += 10;
        if let Some(prev) = last_match {
            if abs_idx == prev + 1 {
                score += 15; // contiguous bonus
            }
        }
        if abs_idx == 0
            || matches!(
                cand_lower.as_bytes().get(abs_idx.saturating_sub(1)),
                Some(b'/') | Some(b'_') | Some(b'-') | Some(b'.')
            )
        {
            score += 12; // boundary bonus
        }
        last_match = Some(abs_idx);
        search_from = abs_idx + qc.len_utf8();
    }

    // Prefer shorter paths slightly on ties.
    score -= (candidate.len() as i64) / 4;
    Some(score)
}

const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB per image

fn image_mime_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Extract `@path/to/image.{png,jpg,jpeg,webp}` tokens that resolve to existing files inside `cwd`.
pub fn detect_image_mentions(input: &str, cwd: &Path) -> Vec<String> {
    let Ok(guard) = openmi_platform::PathGuard::new(cwd) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for token in input.split_whitespace() {
        let Some(raw) = token.strip_prefix('@') else {
            continue;
        };
        let cleaned = raw.trim_end_matches([',', '.', ';', ':', ')', ']']);
        if cleaned.is_empty() {
            continue;
        }
        let candidate = Path::new(cleaned);
        if image_mime_for_path(candidate).is_none() {
            continue;
        }
        if let Ok(resolved) = guard.resolve_existing(candidate) {
            if resolved.is_file() && !found.iter().any(|p| p == cleaned) {
                found.push(cleaned.to_string());
            }
        }
    }
    found
}

/// Load and base64-encode all valid `@image.{png,jpg,jpeg,webp}` mentions inside `cwd` (max 5 MB each).
pub fn load_image_attachments(input: &str, cwd: &Path) -> Vec<openmi_core::ImageAttachment> {
    let Ok(guard) = openmi_platform::PathGuard::new(cwd) else {
        return Vec::new();
    };
    let mut attachments = Vec::new();
    for rel in detect_image_mentions(input, cwd) {
        let candidate = Path::new(&rel);
        let Some(mime) = image_mime_for_path(candidate) else {
            continue;
        };
        let Ok(resolved) = guard.resolve_existing(candidate) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&resolved) {
            let label = candidate
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel.clone());
            attachments.push(openmi_core::ImageAttachment {
                mime_type: mime.to_string(),
                data_base64: openmi_core::encode_base64(&bytes),
                label,
            });
        }
    }
    attachments
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extracts_active_at_mention_and_fuzzy_matches_with_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a git repo marker so `ignore` honors `.gitignore`.
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".gitignore"), "ignored.log\n").unwrap();
        fs::create_dir_all(tmp.path().join("src/tui")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("src/tui/render.rs"), "").unwrap();
        fs::write(tmp.path().join("ignored.log"), "secret").unwrap();

        let files = collect_workspace_files(tmp.path(), 500);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/tui/render.rs".to_string()));
        assert!(!files.contains(&"ignored.log".to_string()));

        let text = "inspect @tuiren";
        let (start, q) = active_mention_query(text, text.len()).unwrap();
        assert_eq!(start, 8);
        assert_eq!(q, "tuiren");

        let matches = fuzzy_match_paths(&files, q, 5);
        assert_eq!(
            matches.first().map(String::as_str),
            Some("src/tui/render.rs")
        );
    }

    #[test]
    fn detects_and_base64_encodes_image_mentions() {
        let tmp = tempfile::tempdir().unwrap();
        let img = tmp.path().join("screen.png");
        fs::write(&img, b"\x89PNG\r\n\x1a\nfake").unwrap();

        let prompt = "check @screen.png and @missing.jpg";
        let detected = detect_image_mentions(prompt, tmp.path());
        assert_eq!(detected, vec!["screen.png"]);

        let atts = load_image_attachments(prompt, tmp.path());
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].mime_type, "image/png");
        assert_eq!(atts[0].label, "screen.png");
        assert!(!atts[0].data_base64.is_empty());
    }
}
