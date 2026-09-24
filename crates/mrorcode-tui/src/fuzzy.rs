//! 零依赖 subsequence fuzzy 评分（fzf 风格）。
//!
//! 子序列匹配 + 加权：前缀/分隔符后/CamelCase 边界命中、连续命中 streak；
//! 起始位置与间隔惩罚、目标长度惩罚。返回 0..u32 分值（0 仅用于空 query），
//! 不匹配返回 None。供对话框过滤与命中字符高亮。
//!
//! 热路径零分配：query 只折叠编译一次，target 直接按 char 迭代，
//! 1 万条目过滤 < 0.5ms（见 benches/render.rs）。

#[inline]
fn fold(c: char) -> char {
    c.to_ascii_lowercase()
}

#[inline]
fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '/' | '\\' | '.' | ':' | '>' | '|')
}

/// 编译一次的 query（小写 char 序列）。
#[derive(Default)]
struct CompiledQuery {
    chars: Vec<char>,
}

impl CompiledQuery {
    fn new(query: &str) -> Self {
        Self {
            chars: query.chars().map(fold).collect(),
        }
    }
}

/// 评分一次匹配；返回 (score, 命中字符在 target 中的 char 索引)。
/// query 为空时匹配一切（score = 0，无高亮位置）。
pub fn score(query: &str, target: &str) -> Option<(u32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    score_inner::<true>(&CompiledQuery::new(query), target)
}

/// score 的零分配版本：query 预编译，供 filter 批量调用。
/// `WANT_POS` 为 false 时跳过 positions Vec 分配（过滤只需要分值）。
fn score_inner<const WANT_POS: bool>(q: &CompiledQuery, target: &str) -> Option<(u32, Vec<usize>)> {
    if q.chars.is_empty() {
        return Some((0, Vec::new()));
    }

    let mut positions = if WANT_POS {
        Vec::with_capacity(q.chars.len())
    } else {
        Vec::new()
    };
    let mut total: i64 = 0;
    let mut qi = 0usize;
    let mut prev: Option<usize> = None;
    let mut streak: i64 = 0;
    let mut prev_c = '\0';
    let mut tlen = 0usize;

    for (ti, raw) in target.chars().enumerate() {
        tlen += 1;
        if qi < q.chars.len() && fold(raw) == q.chars[qi] {
            if WANT_POS {
                positions.push(ti);
            }
            let mut gain: i64 = 1;

            if ti == 0 {
                gain += 16; // 目标起始
            }
            if ti > 0 && is_separator(prev_c) {
                gain += 10; // 分隔符后的词首
            }
            if ti > 0 && prev_c.is_ascii_lowercase() && raw.is_ascii_uppercase() {
                gain += 8; // camelCase 边界
            }

            match prev {
                Some(p) if p + 1 == ti => {
                    streak += 1;
                    gain += 8 * streak; // 连续命中：紧凑匹配显著优先
                }
                Some(p) => {
                    streak = 0;
                    gain -= (ti - p - 1).min(4) as i64; // 间隔惩罚
                }
                None => {
                    streak = 0;
                    gain -= ti as i64 * 2; // 首个命中越靠后越差
                }
            }

            total += gain;
            prev = Some(ti);
            qi += 1;
        }
        prev_c = raw;
    }

    if qi < q.chars.len() {
        return None;
    }

    // 目标比 query 长出的部分轻微惩罚（短而准的优先）
    total -= i64::max(0, tlen as i64 - q.chars.len() as i64) / 4;
    if total < 1 {
        total = 1;
    }
    Some((total as u32, positions))
}

/// 对一批条目过滤+排序，返回 (原索引, score)，按分值降序（稳定，平分保序）。
/// 空 query 时全部保留且保序。query 只编译一次。
pub fn filter<I, S>(query: &str, items: I) -> Vec<(usize, u32)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let compiled = CompiledQuery::new(query);
    let mut out: Vec<(usize, u32)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, s)| score_inner::<false>(&compiled, s.as_ref()).map(|(sc, _)| (i, sc)))
        .collect();
    // 稳定排序：Reverse 键保持平分时原序
    out.sort_by_key(|(_, sc)| std::cmp::Reverse(*sc));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_all() {
        assert_eq!(score("", "anything"), Some((0, vec![])));
        assert_eq!(filter("", ["a", "b"]), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn subsequence_and_miss() {
        assert!(score("abc", "axbxc").is_some());
        assert!(score("abc", "acb").is_none());
        assert!(score("xyz", "abcdef").is_none());
    }

    #[test]
    fn case_insensitive() {
        assert!(score("NEW", "New Session").is_some());
        assert!(score("new", "NEW SESSION").is_some());
    }

    #[test]
    fn prefix_and_boundary_beat_inline() {
        // 词首命中应显著优于行中散落命中
        let boundary = score("tf", "test_file").unwrap().0;
        let inline = score("tf", "atestfzzz").unwrap().0;
        assert!(boundary > inline);

        let prefix = score("new", "new session").unwrap().0;
        let mid = score("new", "a new thing").unwrap().0;
        assert!(prefix > mid);
    }

    #[test]
    fn consecutive_run_scores_higher() {
        let tight = score("abc", "abc").unwrap().0;
        let spread = score("abc", "a__b__c").unwrap().0;
        assert!(tight > spread);
    }

    #[test]
    fn filter_orders_by_score_desc() {
        let items = ["banana", "new file", "new session"];
        let ranked = filter("new", items);
        assert_eq!(ranked[0].0, 1); // "new file" 比 "new session" 短
        assert_eq!(ranked[1].0, 2);
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn positions_are_char_indices() {
        let (_, pos) = score("ns", "new session").unwrap();
        assert_eq!(pos, vec![0, 4]);
    }

    #[test]
    fn multibyte_target_counts_by_chars_not_bytes() {
        // 中文字符按 char 索引匹配
        let r = score("主题", "切换主题");
        assert!(r.is_some());
        let (_, pos) = r.unwrap();
        assert_eq!(pos, vec![2, 3]);
    }
}
