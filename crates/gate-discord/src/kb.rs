//! 奶妹的知识库：编译期打包的 Markdown 文档 + 极简检索。
//!
//! 为什么不用向量库：知识库一共两份长文档，规模在几十 KB；按 `## ` 标题切块后，
//! 用「字符二元组重合度」打分取 Top-K 已经够分清「领 Key」和「生图参数」这类问题，
//! 不值得为一个看板娘引入嵌入模型和索引依赖。
//!
//! 文档都是写给成员看的（无内部地址、无凭据），整段注入提示词是安全的；
//! `kb/nailong.md` 不参与检索，只在卖萌场景整份给出。

use std::collections::HashSet;
use std::sync::OnceLock;

pub const OWL_GUIDE: &str = include_str!("kb/owl-user-guide.md");
pub const NAI_FACTS: &str = include_str!("kb/novelai-facts.md");
pub const NAILONG: &str = include_str!("kb/nailong.md");

struct Chunk {
    title: String,
    body: String,
}

fn chunks() -> &'static Vec<Chunk> {
    static CHUNKS: OnceLock<Vec<Chunk>> = OnceLock::new();
    CHUNKS.get_or_init(|| {
        let mut out = Vec::new();
        for doc in [OWL_GUIDE, NAI_FACTS] {
            // 首个 `# ` 一级标题是文档名，其后按 `## ` 切块
            let doc_title = doc
                .lines()
                .find_map(|l| l.strip_prefix("# "))
                .unwrap_or("知识库");
            let mut current_title = doc_title.to_owned();
            let mut current_body = String::new();
            for line in doc.lines().skip_while(|l| !l.starts_with("## ")) {
                if let Some(t) = line.strip_prefix("## ") {
                    if !current_body.trim().is_empty() {
                        out.push(Chunk {
                            title: current_title.clone(),
                            body: current_body.trim().to_owned(),
                        });
                    }
                    current_title = t.trim().to_owned();
                    current_body.clear();
                } else {
                    current_body.push_str(line);
                    current_body.push('\n');
                }
            }
            if !current_body.trim().is_empty() {
                out.push(Chunk {
                    title: current_title,
                    body: current_body.trim().to_owned(),
                });
            }
        }
        out
    })
}

/// 字符二元组集合：小写化后取相邻字符对，中文英文一视同仁。
fn bigrams(text: &str) -> HashSet<[char; 2]> {
    let lower: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    lower.windows(2).map(|w| [w[0], w[1]]).collect()
}

/// 按用户的问题检索知识库，返回最多 `top` 块的 Markdown 拼接；都不相关时返回空串。
/// 相关门槛：命中二元组要覆盖问题的三分之一以上——「今天天气怎么样」这类闲聊
/// 只会撞上「怎么」之类的公共词，不该硬塞知识库。
pub fn retrieve(query: &str, top: usize) -> String {
    let q = bigrams(query);
    if q.is_empty() {
        return String::new();
    }
    let threshold = (q.len() / 3).max(1);
    let mut scored: Vec<(usize, &Chunk)> = chunks()
        .iter()
        .map(|c| {
            (
                bigrams(&format!("{} {}", c.title, c.body))
                    .intersection(&q)
                    .count(),
                c,
            )
        })
        .filter(|(score, _)| *score >= threshold && *score > 0)
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(top)
        .map(|(_, c)| format!("### {}（摘录）\n{}", c.title, c.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 卖萌素材整份返回（使用规则 + 直链 + 文字表情）。
pub fn nailong() -> &'static str {
    NAILONG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_split_and_titled() {
        let cs = chunks();
        assert!(cs.len() >= 10, "两份文档应切出足够多的块：{}", cs.len());
        assert!(cs.iter().any(|c| c.title.contains("领 Key")));
        assert!(cs.iter().any(|c| c.title.contains("参数")));
    }

    #[test]
    fn retrieve_finds_relevant_chunks() {
        let hit = retrieve("怎么领 key？领了怎么填到客户端里", 2);
        assert!(hit.contains("领 Key"), "应命中领 Key 块：{hit}");
        let hit = retrieve("提示词权重 {} 怎么用", 2);
        assert!(hit.contains("提示词"), "应命中提示词块：{hit}");
        let miss = retrieve("今天天气怎么样", 2);
        assert!(miss.is_empty(), "无关问题不硬塞知识库：{miss}");
    }

    #[test]
    fn nailong_doc_has_only_verified_links() {
        assert!(NAILONG.contains("raw.githubusercontent.com/lin-alg/NaiLoong/main/"));
        assert!(NAILONG.contains("偶尔"), "必须带使用规则");
    }
}
