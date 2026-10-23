//! 线上冒烟（取代旧版的 live_smoke.py 与 smoke_test.py，见标签 py-final）：只做**不出图**的检查，
//! 不消耗任何 Anlas / 免费额度，也不会触发出图间隔。
//!
//! 检查：`/healthz`、`/public/status`、`/public/live`、`/status.json`、`/queue-status`；
//! 以及旧版冒烟里不出图的两条鉴权检查（无 Key / 错 Key 访问 `/v1/me` 必须 401）；
//! 环境变量 `SMOKE_KEY` 有值时再多一条：用这把 Key 访问 `/v1/me` 必须 200。
//! Key 只放在请求头里，**任何输出都不会带上它**（连错误信息里也会被抹掉）。
//!
//! 任何一项 FAIL，退出码非 0。

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Pass,
    Fail,
    Info,
}

impl Flag {
    fn label(self) -> &'static str {
        match self {
            Flag::Pass => "PASS",
            Flag::Fail => "FAIL",
            Flag::Info => "INFO",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub flag: Flag,
    pub name: String,
    pub line: String,
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
}

impl Report {
    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.flag == Flag::Fail)
            .count()
    }
    /// 汇总（逐项结果在检查时已经边跑边打印了）。
    pub fn summary(&self) -> String {
        let mut out = String::from("\n=== 汇总 ===\n");
        for f in [Flag::Fail, Flag::Pass, Flag::Info] {
            out.push_str(&format!(
                "{} {}\n",
                f.label(),
                self.outcomes.iter().filter(|o| o.flag == f).count()
            ));
        }
        out
    }
    pub fn exit_code(&self) -> i32 {
        i32::from(self.failed() > 0)
    }
}

struct Got {
    status: u16,
    content_type: String,
    body: String,
}

struct Smoke<'a> {
    http: reqwest::Client,
    base: String,
    /// 需要从输出里抹掉的字符串（Key）
    secret: Option<&'a str>,
    outcomes: Vec<Outcome>,
}

impl Smoke<'_> {
    fn scrub(&self, s: &str) -> String {
        match self.secret {
            Some(k) if !k.is_empty() => s.replace(k, "***"),
            _ => s.to_string(),
        }
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> std::result::Result<Got, String> {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(k) = bearer {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        match req.send().await {
            Err(e) => {
                let kind = if e.is_timeout() {
                    "超时"
                } else if e.is_connect() {
                    "连不上"
                } else {
                    "请求出错"
                };
                Err(self.scrub(&format!("{kind}：{e}")))
            }
            Ok(r) => {
                let status = r.status().as_u16();
                let content_type = r
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = r.text().await.unwrap_or_default();
                Ok(Got {
                    status,
                    content_type,
                    body,
                })
            }
        }
    }

    fn record(&mut self, flag: Flag, name: &str, got: Option<&Got>, note: &str) {
        let detail = match got {
            Some(g) => format!(
                "[{}] {:28} {:>8}B",
                g.status,
                gate_core::pyc::take_chars(&g.content_type, 28),
                g.body.len()
            ),
            None => String::new(),
        };
        let line = self.scrub(format!("{:4} {:44} {detail} {note}", flag.label(), name).trim_end());
        println!("{line}");
        self.outcomes.push(Outcome {
            flag,
            name: name.to_string(),
            line,
        });
    }

    /// GET + 状态码 + 对 JSON 正文的额外断言。
    async fn check_json(
        &mut self,
        name: &str,
        path: &str,
        bearer: Option<&str>,
        want: u16,
        valid: impl Fn(&Value) -> Result<(), &'static str>,
    ) {
        match self.get(path, bearer).await {
            Err(e) => self.record(Flag::Fail, name, None, &e),
            Ok(g) => {
                let verdict = if g.status != want {
                    Err(format!("期望 {want}"))
                } else if want == 200 {
                    match serde_json::from_str::<Value>(&g.body) {
                        Err(_) => Err("正文不是 JSON".to_string()),
                        Ok(v) => valid(&v).map_err(String::from),
                    }
                } else {
                    Ok(())
                };
                match verdict {
                    Ok(()) => self.record(Flag::Pass, name, Some(&g), ""),
                    Err(why) => self.record(Flag::Fail, name, Some(&g), &why),
                }
            }
        }
    }
}

fn is_object(v: &Value) -> Result<(), &'static str> {
    if v.is_object() {
        Ok(())
    } else {
        Err("正文不是 JSON 对象")
    }
}

/// 跑完所有检查。`key` 为 None 时跳过「带 Key 访问 /v1/me」。
pub async fn run(base: &str, key: Option<&str>, timeout: Duration) -> Result<Report> {
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("nai-ops-smoke/1")
        .build()?;
    let mut s = Smoke {
        http,
        base: base.trim_end_matches('/').to_string(),
        secret: key,
        outcomes: Vec::new(),
    };

    s.check_json("healthz", "/healthz", None, 200, |v| match v.get("ok") {
        Some(Value::Bool(true)) => Ok(()),
        _ => Err("ok 不是 true"),
    })
    .await;
    s.check_json("public/status", "/public/status", None, 200, is_object)
        .await;
    s.check_json("public/live", "/public/live", None, 200, is_object)
        .await;
    s.check_json("status.json", "/status.json", None, 200, |v| {
        if v.get("components").is_some_and(Value::is_array) {
            Ok(())
        } else {
            Err("缺少 components 数组")
        }
    })
    .await;
    // 出图队列的公开状态（原来检查的 /v1/models 随文本接口一起删除了）
    s.check_json("queue-status", "/queue-status", None, 200, |v| {
        if v.get("global").is_some_and(Value::is_object) {
            Ok(())
        } else {
            Err("缺少 global 对象")
        }
    })
    .await;
    s.check_json("v1/me without key -> 401", "/v1/me", None, 401, |_| Ok(()))
        .await;
    s.check_json(
        "v1/me wrong key -> 401",
        "/v1/me",
        Some("nai-definitely-wrong"),
        401,
        |_| Ok(()),
    )
    .await;
    match key {
        Some(k) if !k.is_empty() => {
            s.check_json("v1/me valid key (SMOKE_KEY)", "/v1/me", Some(k), 200, |v| {
                if v.get("today").is_some_and(Value::is_object) {
                    Ok(())
                } else {
                    Err("缺少 today 对象")
                }
            })
            .await;
        }
        _ => s.record(
            Flag::Info,
            "v1/me valid key",
            None,
            "SMOKE_KEY 未设置：跳过带 Key 的检查",
        ),
    }
    Ok(Report {
        outcomes: s.outcomes,
    })
}
