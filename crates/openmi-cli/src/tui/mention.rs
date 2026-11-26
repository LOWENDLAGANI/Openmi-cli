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
}
