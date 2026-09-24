//! 路径安全 gate：所有文件工具把路径限制在 working_dir 内。
//!
//! 解析策略：
//! 1. 绝对路径直接采用，相对路径拼接到 working_dir 下；
//! 2. 对「已存在的最长祖先」做 `canonicalize`（可捕捉符号链接指向外部），
//!    再把剩余（尚不存在）的组件做词法规范化后拼接；
//! 3. 最终路径必须 `starts_with(canonical working_dir)`，否则返回结构化错误。
//!
//! Windows 额外拒绝 UNC / verbatim 前缀（`\\server\share`、`\\?\C:\...`）。

use std::path::{Component, Path, PathBuf};

/// 把用户提供的路径解析为 working_dir 内的绝对路径；越界返回错误字符串。
pub fn resolve_safe(working_dir: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("empty path".to_string());
    }
    let raw_path = Path::new(raw);
    if has_unc_or_verbatim(raw_path) {
        return Err(format!("unsupported path prefix: {raw}"));
    }

    let joined = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        working_dir.join(raw_path)
    };

    let base = canonical_or_lexical(working_dir);
    let target = resolve_existing_ancestor(&joined);

    if !target.starts_with(&base) {
        return Err(format!(
            "path escapes working directory: {} (allowed root: {})",
            raw,
            base.display()
        ));
    }
    Ok(target)
}

/// 解析结果是否为已存在文件（用于 read/edit 区分「文件不存在」与「越界」）。
pub fn resolve_existing_file(working_dir: &Path, raw: &str) -> Result<PathBuf, String> {
    let path = resolve_safe(working_dir, raw)?;
    if !path.exists() {
        return Err(format!("file not found: {raw}"));
    }
    if path.is_dir() {
        return Err(format!("{} is a directory", raw));
    }
    Ok(path)
}

/// canonicalize 失败时回退到词法规范化（目录本身通常存在，此路径基本不触发）。
fn canonical_or_lexical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| lexical_normalize(path))
}

/// 对已存在的最长祖先做 canonicalize，再拼上尚不存在、词法规范化的尾部。
fn resolve_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    let mut existing: Option<&Path> = None;
    for ancestor in path.ancestors().skip(1) {
        if ancestor.exists() {
            existing = Some(ancestor);
            break;
        }
    }
    match existing {
        Some(ancestor) => {
            let canon = ancestor
                .canonicalize()
                .unwrap_or_else(|_| lexical_normalize(ancestor));
            let rest = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
            lexical_normalize(&canon.join(lexical_normalize(rest)))
        }
        None => lexical_normalize(path),
    }
}

/// 纯词法规范化：消除 `.` 与 `..`（不访问文件系统）。
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(windows)]
fn has_unc_or_verbatim(path: &Path) -> bool {
    use std::path::Prefix;
    matches!(
        path.components().next(),
        Some(Component::Prefix(pr))
            if matches!(
                pr.kind(),
                Prefix::UNC(..)
                    | Prefix::VerbatimUNC(..)
                    | Prefix::VerbatimDisk(..)
                    | Prefix::Verbatim(_)
            )
    )
}

#[cfg(not(windows))]
fn has_unc_or_verbatim(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mrorcode-guard-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normal_subpath_is_allowed() {
        let base = temp_dir("normal");
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/a.rs"), "x").unwrap();
        let p = resolve_safe(&base, "src/a.rs").unwrap();
        assert!(p.starts_with(base.canonicalize().unwrap()));
        assert!(p.ends_with("src/a.rs") || p.ends_with("a.rs"));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn parent_dir_escape_is_rejected() {
        let base = temp_dir("dotdot");
        let err = resolve_safe(&base, "../../etc/passwd").unwrap_err();
        assert!(err.contains("escapes working directory"), "{err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn absolute_outside_is_rejected() {
        let base = temp_dir("abs");
        let outside = std::env::temp_dir().join("mrorcode-guard-outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let err = resolve_safe(&base, outside.to_str().unwrap()).unwrap_err();
        assert!(err.contains("escapes working directory"), "{err}");
        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(windows)]
    #[test]
    fn symlink_escape_is_rejected_if_creatable() {
        let base = temp_dir("symlink");
        let outside = temp_dir("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        let link = base.join("link");
        // 普通用户创建目录符号链接需要权限；失败则跳过。
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            std::fs::remove_dir_all(&base).ok();
            std::fs::remove_dir_all(&outside).ok();
            return;
        }
        let err = resolve_safe(&base, "link/secret.txt").unwrap_err();
        assert!(err.contains("escapes working directory"), "{err}");
        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let base = temp_dir("symlink");
        let outside = temp_dir("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let err = resolve_safe(&base, "link/secret.txt").unwrap_err();
        assert!(err.contains("escapes working directory"), "{err}");
        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn nonexistent_write_target_inside_is_allowed() {
        let base = temp_dir("newwrite");
        let p = resolve_safe(&base, "nested/new.txt").unwrap();
        assert!(p.starts_with(base.canonicalize().unwrap()));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn dotdot_that_stays_inside_is_allowed() {
        let base = temp_dir("inside-dotdot");
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        let p = resolve_safe(&base, "a/b/../c.txt").unwrap();
        assert!(p.starts_with(base.canonicalize().unwrap()));
        assert!(p.ends_with("a/c.txt") || p.ends_with("c.txt"));
        std::fs::remove_dir_all(&base).ok();
    }
}
