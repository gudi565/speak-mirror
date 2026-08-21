# 实时表达训练系统 MVP 实施计划（M1 + M2）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 搭出可运行的 Tauri 桌面应用：实时字幕 + 6 条本地规则反馈 + 三栏 UI，覆盖里程碑 M1（技术验证）与 M2（规则引擎 + UI 联动）。

**Architecture:** Rust 侧负责音频采集(cpal) → VAD(silero) → ASR(sherpa-rs streaming zipformer) → 规则引擎，Tauri event 推送前端；React 前端纯渲染。规则引擎是纯逻辑模块，`cargo test` 全覆盖，不依赖模型文件。

**Tech Stack:** Tauri v2 / Rust (cpal 0.15, sherpa-rs 0.6.8, serde) / React 18 + TypeScript + Vite + Tailwind v4。

**Spec:** `docs/superpowers/specs/2026-08-21-expression-trainer-design.md`

**范围说明：** 本计划只做 M1 + M2（到三栏 UI 实时联动可用为止）。M3（LLM 报告 / Markdown 导出 / 设置页）在下一份计划。

## Global Constraints

- 项目目录：`D:\普通话\express-trainer`（仓库子目录；若 sherpa-rs-sys 的 cmake 构建因非 ASCII 路径失败，按 Task 0 末尾的降级方案处理）
- ASR 采样率统一 16000 Hz、单声道、f32
- 规则阈值（来自 spec）：重复检测窗口 20 句 / 相似度 > 0.7；结论缺失连续 5 句；举例缺失观点连续 4 句
- 默认填充词表：然后、就是、那个、呃、嗯、其实、比如说
- Rust 与前端之间的所有结构体序列化用 `camelCase`
- 模型文件不进 git（`.gitignore` 忽略 `express-trainer/models/`）

---

### Task 0: 安装 Rust 工具链

**Files:** 无（系统环境）

**Interfaces:**
- Produces: 后续所有 Rust 任务依赖的 `cargo` / `rustc`（1.75+）

- [ ] **Step 1: 安装 rustup（MSVC Build Tools 2022 已确认存在）**

```powershell
winget install -e --id Rustlang.Rustup --accept-source-agreements --accept-package-agreements
```

- [ ] **Step 2: 刷新当前会话 PATH 并验证**

```powershell
$env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
rustc --version; cargo --version
```

Expected: 输出 1.7x 以上版本号。若仍找不到，重开终端再验。

- [ ] **Step 3: 降级预案（仅在 Task 10 编译 sherpa-rs-sys 因中文路径失败时执行）**

把 `express-trainer` 目录移到 `D:\dev\express-trainer`，其余任务路径相应替换。**不要预先移动**，先尝试原位构建。

---

### Task 1: 脚手架 Tauri + React + TS + Tailwind

**Files:**
- Create: `express-trainer/`（create-tauri-app 生成的标准结构 + tailwind）

**Interfaces:**
- Produces: `npm run tauri dev` 可启动空白窗口；`src-tauri` 为后续 Rust 任务的工作目录

- [ ] **Step 1: 生成模板（非交互）**

```powershell
cd D:\普通话
npm create tauri-app@latest express-trainer -- --template react-ts --manager npm --yes
cd express-trainer
npm install
```

Expected: 生成 `express-trainer/src-tauri`、`src`、`package.json`。

- [ ] **Step 2: 安装 Tailwind v4 与前端依赖**

```powershell
npm install tailwindcss @tailwindcss/vite @tauri-apps/api
```

- [ ] **Step 3: 接入 Tailwind**

`express-trainer/vite.config.ts` 全文替换为：

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
```

`express-trainer/src/index.css` 全文替换为：

```css
@import "tailwindcss";
```

确认 `src/main.tsx` 里 `import "./index.css";`（模板若引入 `App.css` 则改为 index.css 并删除 `App.css`）。

- [ ] **Step 4: 配置 .gitignore 与窗口标题**

在 `express-trainer/.gitignore` 末尾追加：

```
models/
```

`src-tauri/tauri.conf.json` 中 `productName` 改为 `express-trainer`，窗口 `title` 改为 `表达训练系统`。

- [ ] **Step 5: 验证桌面端可启动**

```powershell
cd D:\普通话\express-trainer
npm run tauri dev
```

Expected: 编译成功后弹出应用窗口显示 Vite 模板页。首次编译较久（5-15 分钟）属正常。验证后 Ctrl+C 停止。

- [ ] **Step 6: Commit**

```powershell
cd D:\普通话
git add express-trainer
git commit -m "chore: scaffold tauri + react + tailwind app"
```

---

### Task 2: 规则引擎核心类型

**Files:**
- Create: `express-trainer/src-tauri/src/rules/mod.rs`
- Modify: `express-trainer/src-tauri/src/lib.rs`（注册 `pub mod rules;`）
- Test: 同文件内 `#[cfg(test)]`

**Interfaces:**
- Produces（后续所有规则任务与 Task 8/11 依赖这些签名）:
  - `Sentence { id: u64, text: String, start_ms: u64, end_ms: u64 }`
  - `FeedbackKind { FillerWord, WordPrecision, Repetition, ConclusionMissing, ExampleMissing, Emotion }`
  - `FeedbackEvent { kind: FeedbackKind, sentence_id: Option<u64>, message: String, payload: serde_json::Value }`
  - `SessionContext { sentences: Vec<Sentence>, filler_counts: HashMap<String, u32>, emotion_counts: HashMap<String, u32>, started_at_ms: u64 }`
  - `trait Rule { fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent>; fn name(&self) -> &'static str; }`

- [ ] **Step 1: 写实现与测试（类型骨架简单，一次写完）**

`express-trainer/src-tauri/src/rules/mod.rs`：

```rust
pub mod filler;
pub mod lexicon;
pub mod precision;
pub mod repetition;
pub mod structure;
pub mod emotion;
pub mod engine;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sentence {
    pub id: u64,
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FeedbackKind {
    FillerWord,
    WordPrecision,
    Repetition,
    ConclusionMissing,
    ExampleMissing,
    Emotion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEvent {
    pub kind: FeedbackKind,
    pub sentence_id: Option<u64>,
    pub message: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct SessionContext {
    pub sentences: Vec<Sentence>,
    pub filler_counts: HashMap<String, u32>,
    pub emotion_counts: HashMap<String, u32>,
    pub started_at_ms: u64,
}

pub trait Rule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent>;
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_serializes_to_camel_case() {
        let s = Sentence { id: 1, text: "你好".into(), start_ms: 0, end_ms: 1000 };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["startMs"], 0);
        assert_eq!(v["endMs"], 1000);
    }

    #[test]
    fn feedback_event_serializes_kind_camel_case() {
        let e = FeedbackEvent {
            kind: FeedbackKind::FillerWord,
            sentence_id: Some(3),
            message: "口头禅".into(),
            payload: serde_json::json!({"word": "然后"}),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "fillerWord");
        assert_eq!(v["sentenceId"], 3);
    }
}
```

注意：本任务中 `pub mod filler;` 等子模块声明先加，子模块文件在 Task 3-8 逐个创建；期间 `cargo test` 需在对应文件存在后才能编译。因此本任务 Step 2 先只声明 `pub mod filler;` 等会在同一 commit 前创建空壳文件，每个空壳文件内容为对应模块占位注释行 `// filled in by Task N`。后续任务用全文替换覆盖。

- [ ] **Step 2: 验证测试失败（serde 依赖未加）**

Run: `cd D:\普通话\express-trainer\src-tauri; cargo test`
Expected: FAIL — `serde` unresolved。

- [ ] **Step 3: 添加依赖使编译通过**

`express-trainer/src-tauri/Cargo.toml` 的 `[dependencies]` 追加：

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Run: `cargo test`
Expected: `rules::tests` 两个用例 PASS。

- [ ] **Step 4: Commit**

```powershell
cd D:\普通话
git add express-trainer/src-tauri
git commit -m "feat(rules): core types Sentence/FeedbackEvent/Rule trait"
```

---

### Task 3: 填充词规则 `filler_words`

**Files:**
- Create: `express-trainer/src-tauri/src/rules/filler.rs`（替换空壳）

**Interfaces:**
- Consumes: Task 2 的 `Rule`/`Sentence`/`SessionContext`/`FeedbackEvent`
- Produces: `FillerWordsRule::default()` / `FillerWordsRule::new(words: Vec<String>)`；事件 `kind = FillerWord`，`payload = { "word": "然后", "countInSentence": 2, "totalCount": 5, "perMinute": 2.4 }`；规则只读 `ctx.filler_counts`，累计写回由 Task 8 engine 统一做

- [ ] **Step 1: 写失败测试**

`filler.rs`：

```rust
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const DEFAULT_FILLERS: &[&str] = &["然后", "就是", "那个", "呃", "嗯", "其实", "比如说"];

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms }
    }

    #[test]
    fn detects_each_filler_occurrence() {
        let mut rule = FillerWordsRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "然后我想说然后就是", 60_000), &ctx);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload["word"], "然后");
        assert_eq!(events[1].payload["word"], "就是");
    }

    #[test]
    fn no_event_when_clean() {
        let mut rule = FillerWordsRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "今天介绍这个系统的三个功能", 60_000), &ctx);
        assert!(events.is_empty());
    }

    #[test]
    fn computes_per_minute_from_session_elapsed() {
        let mut rule = FillerWordsRule::default();
        let mut ctx = SessionContext::default();
        ctx.filler_counts.insert("然后".into(), 4); // 之前已累计 4 次
        let events = rule.on_sentence(&sent(2, "然后继续", 120_000), &ctx);
        assert_eq!(events[0].payload["totalCount"], 5);
        assert_eq!(events[0].payload["perMinute"], 2.5);
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test filler`
Expected: FAIL — `FillerWordsRule` 未定义。

- [ ] **Step 3: 实现（追加到 `filler.rs` 顶部 use 之后、tests 之前）**

```rust
pub struct FillerWordsRule {
    words: Vec<String>,
}

impl Default for FillerWordsRule {
    fn default() -> Self {
        Self::new(DEFAULT_FILLERS.iter().map(|s| s.to_string()).collect())
    }
}

impl FillerWordsRule {
    pub fn new(words: Vec<String>) -> Self {
        Self { words }
    }
}

impl Rule for FillerWordsRule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let prev_total: u32 = ctx.filler_counts.values().sum();
        let mut total = prev_total;
        let mut events = Vec::new();
        let mut in_sentence: std::collections::HashMap<&str, u32> = Default::default();
        for word in &self.words {
            let occurrences = sentence.text.matches(word.as_str()).count() as u32;
            for _ in 0..occurrences {
                *in_sentence.entry(word).or_insert(0) += 1;
                total += 1;
                let elapsed_min =
                    sentence.end_ms.saturating_sub(ctx.started_at_ms) as f64 / 60_000.0;
                let per_minute = if elapsed_min > 0.0 {
                    (total as f64 / elapsed_min * 10.0).round() / 10.0
                } else {
                    0.0
                };
                events.push(FeedbackEvent {
                    kind: FeedbackKind::FillerWord,
                    sentence_id: Some(sentence.id),
                    message: format!("口头禅「{}」", word),
                    payload: serde_json::json!({
                        "word": word,
                        "countInSentence": in_sentence[word.as_str()],
                        "totalCount": total,
                        "perMinute": per_minute,
                    }),
                });
            }
        }
        events
    }

    fn name(&self) -> &'static str {
        "filler_words"
    }
}
```

- [ ] **Step 4: 验证通过**

Run: `cargo test filler`
Expected: 3 个用例 PASS。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/filler.rs
git commit -m "feat(rules): filler word detection with per-minute rate"
```

---

### Task 4: 词汇精确度规则 `word_precision`

**Files:**
- Create: `express-trainer/src-tauri/src/rules/precision.rs`（替换空壳）
- Create: `express-trainer/src-tauri/src/rules/lexicon.rs`（替换空壳）

**Interfaces:**
- Produces: `PrecisionRule::default()`；事件 `kind = WordPrecision`，`payload = { "original": "想", "alternatives": ["渴望","期待","向往"] }`；`lexicon::REPLACEMENTS: &[(&str, &[&str])]`

- [ ] **Step 1: 写失败测试与词典数据**

`lexicon.rs`：

```rust
/// 笼统词 → 更精确的替换建议（MVP 内置小词典，后续可从情感词库扩充）
pub static REPLACEMENTS: &[(&str, &[&str])] = &[
    ("想", &["渴望", "期待", "向往"]),
    ("很多", &["大量", "海量", "充裕"]),
    ("很好", &["出色", "扎实", "亮眼"]),
    ("厉害", &["强大", "高效", "硬核"]),
    ("东西", &["作品", "方案", "产物"]),
    ("事情", &["问题", "议题", "任务"]),
    ("普通", &["常规", "平庸", "一般"]),
    ("不错", &["可圈可点", "扎实", "超预期"]),
    ("快", &["敏捷", "迅速", "即时"]),
    ("重要", &["关键", "核心", "举足轻重"]),
];
```

`precision.rs`：

```rust
use super::lexicon::REPLACEMENTS;
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::HashSet;

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: 1000 }
    }

    #[test]
    fn suggests_alternatives_for_vague_words() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "我想做一个很好的东西"), &ctx);
        let originals: Vec<&str> = events
            .iter()
            .map(|e| e.payload["original"].as_str().unwrap())
            .collect();
        assert!(originals.contains(&"想"));
        assert!(originals.contains(&"很好"));
        assert!(originals.contains(&"东西"));
        let first = events.iter().find(|e| e.payload["original"] == "想").unwrap();
        assert_eq!(first.payload["alternatives"][0], "渴望");
    }

    #[test]
    fn suggests_each_word_at_most_once_per_session() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        assert_eq!(rule.on_sentence(&sent(1, "我想要这个"), &ctx).len(), 1);
        assert!(rule.on_sentence(&sent(2, "我还想要那个"), &ctx).is_empty());
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test precision`
Expected: FAIL — `PrecisionRule` 未定义。

- [ ] **Step 3: 实现（追加在 `precision.rs` 的 use 之后、tests 之前）**

```rust
pub struct PrecisionRule {
    suggested: HashSet<&'static str>,
}

impl Default for PrecisionRule {
    fn default() -> Self {
        Self { suggested: HashSet::new() }
    }
}

impl Rule for PrecisionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        for (word, alternatives) in REPLACEMENTS {
            if self.suggested.contains(word) {
                continue;
            }
            if sentence.text.contains(word) {
                self.suggested.insert(word);
                events.push(FeedbackEvent {
                    kind: FeedbackKind::WordPrecision,
                    sentence_id: Some(sentence.id),
                    message: format!("「{}」可以更精确", word),
                    payload: serde_json::json!({
                        "original": word,
                        "alternatives": alternatives,
                    }),
                });
            }
        }
        events
    }

    fn name(&self) -> &'static str {
        "word_precision"
    }
}
```

- [ ] **Step 4: 验证通过**

Run: `cargo test precision`
Expected: 2 个用例 PASS。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/precision.rs express-trainer/src-tauri/src/rules/lexicon.rs
git commit -m "feat(rules): word precision suggestions with session dedup"
```

---

### Task 5: 重复检测规则 `repetition`

**Files:**
- Create: `express-trainer/src-tauri/src/rules/repetition.rs`（替换空壳）

**Interfaces:**
- Produces: `RepetitionRule::default()`（窗口 20 句、阈值 0.7，常量 `WINDOW: usize = 20`、`THRESHOLD: f64 = 0.7`）；事件 `kind = Repetition`，`payload = { "similarTo": 3, "similarity": 0.82, "text": "..." }`；相似度函数 `pub fn char_bigram_jaccard(a: &str, b: &str) -> f64`

- [ ] **Step 1: 写失败测试**

`repetition.rs`：

```rust
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const WINDOW: usize = 20;
pub const THRESHOLD: f64 = 0.7;

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn bigram_jaccard_identical_is_one() {
        assert_eq!(char_bigram_jaccard("今天天气不错", "今天天气不错"), 1.0);
    }

    #[test]
    fn bigram_jaccard_disjoint_is_zero() {
        assert_eq!(char_bigram_jaccard("abcde", "xyz"), 0.0);
    }

    #[test]
    fn flags_near_duplicate_sentence() {
        let mut rule = RepetitionRule::default();
        let mut ctx = SessionContext::default();
        ctx.sentences.push(sent(1, "这个系统可以实时分析你的表达问题"));
        let events = rule.on_sentence(&sent(2, "这个系统可以实时分析你的表达问题啊"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["similarTo"], 1);
    }

    #[test]
    fn ignores_different_sentence() {
        let mut rule = RepetitionRule::default();
        let mut ctx = SessionContext::default();
        ctx.sentences.push(sent(1, "这个系统可以实时分析你的表达问题"));
        let events = rule.on_sentence(&sent(2, "晚饭吃什么比较好呢"), &ctx);
        assert!(events.is_empty());
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test repetition`
Expected: FAIL — `RepetitionRule` / `char_bigram_jaccard` 未定义。

- [ ] **Step 3: 实现（追加在 use 之后、tests 之前）**

```rust
use std::collections::HashSet;

pub struct RepetitionRule;

impl Default for RepetitionRule {
    fn default() -> Self {
        Self
    }
}

/// 字符 bigram 的 Jaccard 相似度；任一字符串不足 2 字时按是否完全相等处理
pub fn char_bigram_jaccard(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        chars
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    };
    let (sa, sb) = (bigrams(a), bigrams(b));
    if sa.is_empty() || sb.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    inter / union
}

impl Rule for RepetitionRule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let recent = ctx
            .sentences
            .iter()
            .rev()
            .take(WINDOW)
            .filter(|s| s.id != sentence.id);
        let mut events = Vec::new();
        for prev in recent {
            let sim = char_bigram_jaccard(&sentence.text, &prev.text);
            if sim > THRESHOLD {
                events.push(FeedbackEvent {
                    kind: FeedbackKind::Repetition,
                    sentence_id: Some(sentence.id),
                    message: "这句话已经说过一遍了".into(),
                    payload: serde_json::json!({
                        "similarTo": prev.id,
                        "similarity": (sim * 100.0).round() / 100.0,
                        "text": prev.text,
                    }),
                });
                break; // 一句只提醒一次
            }
        }
        events
    }

    fn name(&self) -> &'static str {
        "repetition"
    }
}
```

- [ ] **Step 4: 验证通过**

Run: `cargo test repetition`
Expected: 4 个用例 PASS。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/repetition.rs
git commit -m "feat(rules): near-duplicate sentence detection via bigram jaccard"
```

---

### Task 6: 结构规则 `conclusion_missing` + `example_missing`

**Files:**
- Create: `express-trainer/src-tauri/src/rules/structure.rs`（替换空壳）

**Interfaces:**
- Produces: `ConclusionMissingRule::default()`（阈值 `CONCLUSION_STREAK: usize = 5`）、`ExampleMissingRule::default()`（阈值 `OPINION_STREAK: usize = 4`）；两条规则各自持有 streak 状态；标记词常量 `CONCLUSION_MARKERS`、`OPINION_MARKERS`、`EXAMPLE_MARKERS`

- [ ] **Step 1: 写失败测试**

`structure.rs`：

```rust
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const CONCLUSION_STREAK: usize = 5;
pub const OPINION_STREAK: usize = 4;

pub const CONCLUSION_MARKERS: &[&str] = &["所以", "总之", "结论是", "我的观点是", "一句话总结"];
pub const OPINION_MARKERS: &[&str] = &["我觉得", "我认为", "应该"];
pub const EXAMPLE_MARKERS: &[&str] = &["比如", "举个例子", "就像"];

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn conclusion_nudge_after_five_descriptive_sentences() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=4 {
            assert!(rule.on_sentence(&sent(i, "这里有一个很细节的描述"), &ctx).is_empty());
        }
        let events = rule.on_sentence(&sent(5, "继续补充更多的细节内容"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::ConclusionMissing);
    }

    #[test]
    fn conclusion_marker_resets_streak() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=4 {
            rule.on_sentence(&sent(i, "细节描述"), &ctx);
        }
        assert!(rule.on_sentence(&sent(5, "所以这就是结论"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(6, "又开始描述"), &ctx).is_empty());
    }

    #[test]
    fn example_nudge_after_four_opinions_without_example() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            assert!(rule.on_sentence(&sent(i, "我觉得这个方向没问题"), &ctx).is_empty());
        }
        let events = rule.on_sentence(&sent(4, "我认为还应该继续推进"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::ExampleMissing);
    }

    #[test]
    fn example_marker_resets_opinion_streak() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            rule.on_sentence(&sent(i, "我觉得这样更好"), &ctx);
        }
        assert!(rule.on_sentence(&sent(4, "举个例子来说明一下"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(5, "我觉得还要继续"), &ctx).is_empty());
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test structure`
Expected: FAIL — 两个 Rule 未定义。

- [ ] **Step 3: 实现（追加在常量之后、tests 之前）**

```rust
fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| text.contains(m))
}

pub struct ConclusionMissingRule {
    streak: usize,
}

impl Default for ConclusionMissingRule {
    fn default() -> Self {
        Self { streak: 0 }
    }
}

impl Rule for ConclusionMissingRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if contains_any(&sentence.text, CONCLUSION_MARKERS) {
            self.streak = 0;
            return vec![];
        }
        self.streak += 1;
        if self.streak == CONCLUSION_STREAK {
            self.streak = 0; // 提醒后重新计，避免连刷
            return vec![FeedbackEvent {
                kind: FeedbackKind::ConclusionMissing,
                sentence_id: Some(sentence.id),
                message: "描述已持续很久，该给结论了".into(),
                payload: serde_json::json!({ "streak": CONCLUSION_STREAK }),
            }];
        }
        vec![]
    }

    fn name(&self) -> &'static str {
        "conclusion_missing"
    }
}

pub struct ExampleMissingRule {
    opinion_streak: usize,
}

impl Default for ExampleMissingRule {
    fn default() -> Self {
        Self { opinion_streak: 0 }
    }
}

impl Rule for ExampleMissingRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if contains_any(&sentence.text, EXAMPLE_MARKERS) {
            self.opinion_streak = 0;
            return vec![];
        }
        if contains_any(&sentence.text, OPINION_MARKERS) {
            self.opinion_streak += 1;
        } else {
            self.opinion_streak = 0;
        }
        if self.opinion_streak == OPINION_STREAK {
            self.opinion_streak = 0;
            return vec![FeedbackEvent {
                kind: FeedbackKind::ExampleMissing,
                sentence_id: Some(sentence.id),
                message: "连续输出观点了，举个例子会更有画面感".into(),
                payload: serde_json::json!({ "streak": OPINION_STREAK }),
            }];
        }
        vec![]
    }

    fn name(&self) -> &'static str {
        "example_missing"
    }
}
```

- [ ] **Step 4: 验证通过**

Run: `cargo test structure`
Expected: 4 个用例 PASS。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/structure.rs
git commit -m "feat(rules): conclusion-missing and example-missing nudges"
```

---

### Task 7: 情感词库规则 `emotion_lexicon`

**Files:**
- Create: `express-trainer/src-tauri/src/rules/emotion.rs`（替换空壳）

**Interfaces:**
- Produces: `EmotionRule::default()`；事件 `kind = Emotion`，每命中一个情感词发一条，`payload = { "word": "开心", "category": "喜" }`；`lexicon` 常量 `EMOTION_WORDS: &[(&str, &str)]`（词, 类别）；类别用七类：喜、怒、哀、惧、恶、惊、好。规则不写 `ctx.emotion_counts`（由 Task 8 engine 落地）

- [ ] **Step 1: 写失败测试**

`emotion.rs`：

```rust
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

/// MVP 内置小型情感词表（词, 类别）。DUTIR 词库作为可选下载项，后续接入。
pub static EMOTION_WORDS: &[(&str, &str)] = &[
    ("开心", "喜"), ("高兴", "喜"), ("喜悦", "喜"), ("兴奋", "喜"),
    ("愤怒", "怒"), ("生气", "怒"), ("恼火", "怒"), ("气死", "怒"),
    ("难过", "哀"), ("伤心", "哀"), ("失落", "哀"), ("遗憾", "哀"),
    ("害怕", "惧"), ("担心", "惧"), ("紧张", "惧"), ("焦虑", "惧"),
    ("讨厌", "恶"), ("恶心", "恶"), ("烦", "恶"), ("厌恶", "恶"),
    ("惊讶", "惊"), ("震惊", "惊"), ("没想到", "惊"), ("意外", "惊"),
    ("喜欢", "好"), ("佩服", "好"), ("信任", "好"), ("感谢", "好"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn detects_emotion_words_with_category() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "我真的很开心但也有点紧张"), &ctx);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload["category"], "喜");
        assert_eq!(events[1].payload["category"], "惧");
    }

    #[test]
    fn no_event_for_neutral_sentence() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "这个按钮在页面右上角"), &ctx).is_empty());
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test emotion`
Expected: FAIL — `EmotionRule` 未定义。

- [ ] **Step 3: 实现（追加在词表之后、tests 之前）**

```rust
pub struct EmotionRule;

impl Default for EmotionRule {
    fn default() -> Self {
        Self
    }
}

impl Rule for EmotionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        EMOTION_WORDS
            .iter()
            .filter(|(word, _)| sentence.text.contains(word))
            .map(|(word, category)| FeedbackEvent {
                kind: FeedbackKind::Emotion,
                sentence_id: Some(sentence.id),
                message: format!("情感词「{}」（{}）", word, category),
                payload: serde_json::json!({ "word": word, "category": category }),
            })
            .collect()
    }

    fn name(&self) -> &'static str {
        "emotion_lexicon"
    }
}
```

- [ ] **Step 4: 验证通过**

Run: `cargo test emotion`
Expected: 2 个用例 PASS。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/emotion.rs
git commit -m "feat(rules): emotion lexicon tagging with small built-in table"
```

---

### Task 8: 规则引擎装配 `RuleEngine` + 会话统计

**Files:**
- Create: `express-trainer/src-tauri/src/rules/engine.rs`（替换空壳）

**Interfaces:**
- Consumes: Task 3-7 全部规则
- Produces（Task 10/11 依赖）:
  - `RuleEngine::new()`（装配 6 条规则，started_at_ms 由 `start(now_ms)` 设置）
  - `engine.start(now_ms: u64)`
  - `engine.ingest(sentence: Sentence) -> Vec<FeedbackEvent>`（先跑规则，再把句子 push 进 ctx、落地 filler/emotion 计数）
  - `engine.snapshot() -> SessionSnapshot`
  - `SessionSnapshot { sentence_count: u64, filler_counts: Vec<(String, u32)>, filler_per_minute: f64, emotion_counts: Vec<(String, u32)>, duration_ms: u64 }`（Serialize, camelCase）

- [ ] **Step 1: 写失败测试**

`engine.rs`：

```rust
use super::emotion::EmotionRule;
use super::filler::FillerWordsRule;
use super::precision::PrecisionRule;
use super::repetition::RepetitionRule;
use super::structure::{ConclusionMissingRule, ExampleMissingRule};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub sentence_count: u64,
    pub filler_counts: Vec<(String, u32)>,
    pub filler_per_minute: f64,
    pub emotion_counts: Vec<(String, u32)>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms: end_ms - 1000, end_ms }
    }

    #[test]
    fn ingest_runs_all_rules_and_accumulates() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let events = engine.ingest(sent(1, "然后我真的很开心", 60_000));
        let kinds: Vec<FeedbackKind> = events.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&FeedbackKind::FillerWord));
        assert!(kinds.contains(&FeedbackKind::Emotion));
        let snap = engine.snapshot();
        assert_eq!(snap.sentence_count, 1);
        assert_eq!(snap.filler_counts, vec![("然后".to_string(), 1)]);
        assert_eq!(snap.emotion_counts, vec![("喜".to_string(), 1)]);
    }

    #[test]
    fn repetition_rule_sees_previous_sentences_via_ctx() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "这个系统可以实时分析你的表达问题", 60_000));
        let events = engine.ingest(sent(2, "这个系统可以实时分析你的表达问题啊", 120_000));
        assert!(events.iter().any(|e| e.kind == FeedbackKind::Repetition));
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后", 60_000));
        let v = serde_json::to_value(engine.snapshot()).unwrap();
        assert_eq!(v["sentenceCount"], 1);
        assert!(v.get("fillerPerMinute").is_some());
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test engine`
Expected: FAIL — `RuleEngine` 未定义。

- [ ] **Step 3: 实现（追加在 SessionSnapshot 之后、tests 之前）**

```rust
pub struct RuleEngine {
    rules: Vec<Box<dyn Rule>>,
    ctx: SessionContext,
    last_end_ms: u64,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self {
            rules: vec![
                Box::new(FillerWordsRule::default()),
                Box::new(PrecisionRule::default()),
                Box::new(RepetitionRule::default()),
                Box::new(ConclusionMissingRule::default()),
                Box::new(ExampleMissingRule::default()),
                Box::new(EmotionRule::default()),
            ],
            ctx: SessionContext::default(),
            last_end_ms: 0,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.ctx.started_at_ms = now_ms;
    }

    pub fn ingest(&mut self, sentence: Sentence) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        for rule in &mut self.rules {
            events.extend(rule.on_sentence(&sentence, &self.ctx));
        }
        // 落地统计：规则只读 ctx，由 engine 统一累计
        for e in &events {
            match e.kind {
                FeedbackKind::FillerWord => {
                    let word = e.payload["word"].as_str().unwrap_or_default().to_string();
                    *self.ctx.filler_counts.entry(word).or_insert(0) += 1;
                }
                FeedbackKind::Emotion => {
                    let cat = e.payload["category"].as_str().unwrap_or_default().to_string();
                    *self.ctx.emotion_counts.entry(cat).or_insert(0) += 1;
                }
                _ => {}
            }
        }
        self.last_end_ms = sentence.end_ms;
        self.ctx.sentences.push(sentence);
        events
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let total_fillers: u32 = self.ctx.filler_counts.values().sum();
        let duration_ms = self.last_end_ms.saturating_sub(self.ctx.started_at_ms);
        let minutes = duration_ms as f64 / 60_000.0;
        let filler_per_minute = if minutes > 0.0 {
            (total_fillers as f64 / minutes * 10.0).round() / 10.0
        } else {
            0.0
        };
        let sort_desc =
            |m: &std::collections::HashMap<String, u32>| -> Vec<(String, u32)> {
                let mut v: Vec<(String, u32)> = m.clone().into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                v
            };
        SessionSnapshot {
            sentence_count: self.ctx.sentences.len() as u64,
            filler_counts: sort_desc(&self.ctx.filler_counts),
            filler_per_minute,
            emotion_counts: sort_desc(&self.ctx.emotion_counts),
            duration_ms,
        }
    }
}
```

- [ ] **Step 4: 验证通过（全量规则测试）**

Run: `cargo test rules`
Expected: 全部用例 PASS（Task 2-8 累计 15 个左右）。

- [ ] **Step 5: Commit**

```powershell
git add express-trainer/src-tauri/src/rules/engine.rs
git commit -m "feat(rules): RuleEngine pipeline with session stats snapshot"
```

---

### Task 9: 模型下载脚本 + sherpa-rs / cpal 依赖引入

**Files:**
- Create: `express-trainer/scripts/download-models.ps1`
- Modify: `express-trainer/src-tauri/Cargo.toml`

**Interfaces:**
- Produces: `models/silero_vad.onnx` + `models/zipformer/`（encoder/decoder/joiner/tokens.txt）；`cargo build` 通过且链接 sherpa-onnx 成功

- [ ] **Step 1: 添加依赖**

`express-trainer/src-tauri/Cargo.toml` 的 `[dependencies]` 追加：

```toml
cpal = "0.15"
sherpa-rs = "0.6.8"
```

- [ ] **Step 2: 验证编译（sherpa-rs-sys 会下载/链接预编译 sherpa-onnx，首次较久）**

Run: `cd D:\普通话\express-trainer\src-tauri; cargo build`
Expected: BUILD SUCCESS。若 cmake/cc 报路径相关错误且路径含中文 → 执行 Task 0 Step 3 降级预案。

- [ ] **Step 3: 写模型下载脚本**

`express-trainer/scripts/download-models.ps1`：

```powershell
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$root = Split-Path -Parent $PSScriptRoot
$models = Join-Path $root "models"
$zip = Join-Path $models "zipformer"
New-Item -ItemType Directory -Force -Path $zip | Out-Null

$vad = Join-Path $models "silero_vad.onnx"
if (-not (Test-Path $vad)) {
    Write-Host "下载 silero_vad.onnx ..."
    Invoke-WebRequest -Uri "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx" -OutFile $vad
}

$pkg = Join-Path $models "zipformer-bilingual.tar.bz2"
if (-not (Test-Path (Join-Path $zip "tokens.txt"))) {
    Write-Host "下载 streaming zipformer 中英双语模型（约 600MB，请耐心）..."
    Invoke-WebRequest -Uri "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2" -OutFile $pkg
    Write-Host "解压 ..."
    tar -xjf $pkg -C $models
    $dir = Join-Path $models "sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20"
    Copy-Item (Join-Path $dir "encoder-epoch-99-avg-1.onnx") (Join-Path $zip "encoder.onnx")
    Copy-Item (Join-Path $dir "decoder-epoch-99-avg-1.onnx") (Join-Path $zip "decoder.onnx")
    Copy-Item (Join-Path $dir "joiner-epoch-99-avg-1.onnx") (Join-Path $zip "joiner.onnx")
    Copy-Item (Join-Path $dir "tokens.txt") (Join-Path $zip "tokens.txt")
    Remove-Item $pkg
    Remove-Item -Recurse -Force $dir
}
Write-Host "模型就绪：$models"
```

- [ ] **Step 4: 运行下载并验证文件齐备**

```powershell
cd D:\普通话\express-trainer
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
Test-Path models\silero_vad.onnx; Test-Path models\zipformer\tokens.txt
```

Expected: 两个 True。模型文件已被 `.gitignore` 忽略，不进提交。

- [ ] **Step 5: Commit**

```powershell
cd D:\普通话
git add express-trainer/scripts express-trainer/src-tauri/Cargo.toml express-trainer/src-tauri/Cargo.lock
git commit -m "chore: add sherpa-rs/cpal deps and model download script"
```

---

### Task 10: 音频采集 + 重采样 + VAD/ASR 会话线程

**Files:**
- Create: `express-trainer/src-tauri/src/audio.rs`
- Create: `express-trainer/src-tauri/src/session.rs`
- Modify: `express-trainer/src-tauri/src/lib.rs`（加 `pub mod audio; pub mod session;`）
- Test: `audio.rs` 内 `#[cfg(test)]`（重采样纯函数可测；采集与 ASR 不做自动化测试）

**Interfaces:**
- Produces（Task 11 依赖）:
  - `audio::start_capture() -> Result<AudioCapture, String>`；`AudioCapture { stream: cpal::Stream, rx: std::sync::mpsc::Receiver<Vec<f32>>, sample_rate: u32, channels: u16 }`
  - `audio::resample_to_16k_mono(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32>`
  - `session::run_session(app: tauri::AppHandle, stop_rx: Receiver<()>, engine: Arc<Mutex<RuleEngine>>, models_dir: PathBuf) -> Result<(), String>`（阻塞，线程内运行）
  - 发出的事件：`partial_transcript { text }`、`sentence_final (Sentence)`、`analysis_update { events: Vec<FeedbackEvent>, snapshot: SessionSnapshot }`

- [ ] **Step 1: 写重采样失败测试**

`audio.rs`：

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct AudioCapture {
    pub stream: cpal::Stream,
    pub rx: std::sync::mpsc::Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_averages_channels() {
        // 左右声道各一个采样对
        let mono = resample_to_16k_mono(&[1.0, 0.0, 0.5, 0.5], 16_000, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn downsamples_48k_to_16k_by_third() {
        let input: Vec<f32> = (0..480).map(|i| i as f32).collect();
        let out = resample_to_16k_mono(&input, 48_000, 1);
        assert_eq!(out.len(), 160);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn passthrough_when_already_16k_mono() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_to_16k_mono(&input, 16_000, 1), input);
    }
}
```

- [ ] **Step 2: 验证失败**

Run: `cargo test audio`
Expected: FAIL — `resample_to_16k_mono` 未定义（且 cpal traits 已可用）。

- [ ] **Step 3: 实现 `audio.rs`（追加在 tests 之前）**

```rust
/// 混成立体声为单声道 + 线性插值重采样到 16kHz
pub fn resample_to_16k_mono(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    if src_rate == 16_000 {
        return mono;
    }
    let ratio = src_rate as f64 / 16_000.0;
    let out_len = (mono.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = mono[idx.min(mono.len() - 1)];
            let b = mono[(idx + 1).min(mono.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// 打开默认输入设备，回调把原始 f32 帧推入 channel
pub fn start_capture() -> Result<AudioCapture, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("找不到可用的麦克风设备")?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败: {e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let config: cpal::StreamConfig = supported.into();

    let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _| {
                let _ = tx.send(data.to_vec());
            },
            |e| eprintln!("audio capture error: {e}"),
            None,
        )
        .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?;
    stream.play().map_err(|e| format!("启动音频流失败: {e}"))?;
    Ok(AudioCapture { stream, rx, sample_rate, channels })
}
```

- [ ] **Step 4: 验证重采样测试通过**

Run: `cargo test audio`
Expected: 3 个用例 PASS（`start_capture` 不在测试中调用）。

- [ ] **Step 5: 实现 `session.rs`（VAD + ASR + 规则引擎 + 事件推送）**

```rust
use crate::audio::{resample_to_16k_mono, start_capture};
use crate::rules::engine::RuleEngine;
use crate::rules::Sentence;
use sherpa_rs::silero_vad::{SileroVad, SileroVadConfig};
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

pub fn run_session(
    app: tauri::AppHandle,
    stop_rx: Receiver<()>,
    engine: Arc<Mutex<RuleEngine>>,
    models_dir: PathBuf,
) -> Result<(), String> {
    let vad_cfg = SileroVadConfig {
        model: models_dir.join("silero_vad.onnx").to_string_lossy().into_owned(),
        min_silence_duration: 0.5,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        threshold: 0.5,
        sample_rate: 16_000,
        window_size: 512,
        provider: None,
        num_threads: Some(1),
        debug: false,
    };
    let mut vad = SileroVad::new(vad_cfg, 30.0).map_err(|e| format!("VAD 初始化失败: {e}"))?;

    let zip = models_dir.join("zipformer");
    let asr_cfg = TransducerConfig {
        encoder: zip.join("encoder.onnx").to_string_lossy().into_owned(),
        decoder: zip.join("decoder.onnx").to_string_lossy().into_owned(),
        joiner: zip.join("joiner.onnx").to_string_lossy().into_owned(),
        tokens: zip.join("tokens.txt").to_string_lossy().into_owned(),
        num_threads: 2,
        sample_rate: 16_000,
        feature_dim: 80,
        decoding_method: "greedy_search".into(),
        hotwords_file: String::new(),
        hotwords_score: 1.5,
        modeling_unit: String::new(),
        bpe_vocab: String::new(),
        blank_penalty: 0.0,
        model_type: "zipformer".into(),
        debug: false,
        provider: None,
    };
    let mut recognizer =
        TransducerRecognizer::new(asr_cfg).map_err(|e| format!("ASR 初始化失败（模型缺失？请先运行 scripts/download-models.ps1）: {e}"))?;

    let capture = start_capture()?;
    let started = Instant::now();
    let mut sentence_id: u64 = 0;
    let mut current_segment: Vec<f32> = Vec::new();
    let mut last_partial = Instant::now();

    engine.lock().unwrap().start(0);

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }
        match capture.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                let samples = resample_to_16k_mono(&chunk, capture.sample_rate, capture.channels);
                current_segment.extend_from_slice(&samples);
                vad.accept_waveform(samples);

                // partial：说话中每 600ms 对当前段跑一次识别
                if vad.is_speech() && last_partial.elapsed() > Duration::from_millis(600) {
                    last_partial = Instant::now();
                    let text = recognizer.transcribe(16_000, &current_segment);
                    if !text.trim().is_empty() {
                        let _ = app.emit("partial_transcript", serde_json::json!({ "text": text }));
                    }
                }

                // 句子定稿
                while !vad.is_empty() {
                    let seg = vad.front();
                    vad.pop();
                    let text = recognizer.transcribe(16_000, &seg.samples);
                    let text = text.trim().to_string();
                    current_segment.clear();
                    if text.is_empty() {
                        continue;
                    }
                    sentence_id += 1;
                    let end_ms = started.elapsed().as_millis() as u64;
                    let sentence = Sentence {
                        id: sentence_id,
                        text,
                        start_ms: end_ms.saturating_sub(seg.samples.len() as u64 / 16),
                        end_ms,
                    };
                    let (events, snapshot) = {
                        let mut eng = engine.lock().unwrap();
                        let events = eng.ingest(sentence.clone());
                        (events, eng.snapshot())
                    };
                    let _ = app.emit("sentence_final", &sentence);
                    let _ = app.emit(
                        "analysis_update",
                        serde_json::json!({ "events": events, "snapshot": snapshot }),
                    );
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(capture.stream);
    Ok(())
}
```

`lib.rs` 顶部追加：

```rust
pub mod audio;
pub mod rules;
pub mod session;
```

- [ ] **Step 6: 验证编译与全量测试**

Run: `cargo test`
Expected: 编译 SUCCESS，全部测试 PASS（session 无自动化测试，仅要求编译通过）。

- [ ] **Step 7: Commit**

```powershell
cd D:\普通话
git add express-trainer/src-tauri
git commit -m "feat: audio capture, resampler, vad+asr session worker"
```

---

### Task 11: Tauri commands 与会话状态接线

**Files:**
- Modify: `express-trainer/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 10 的 `run_session`、Task 8 的 `RuleEngine`/`SessionSnapshot`
- Produces（前端 Task 12 依赖）: commands `start_session() -> Result<(), String>`、`stop_session() -> SessionSnapshot`、`get_snapshot() -> SessionSnapshot`；AppState 管理引擎与停止信号

- [ ] **Step 1: 实现 `lib.rs`（在保留模板 `greet` 的基础上追加，或直接全文替换为下面内容）**

```rust
pub mod audio;
pub mod rules;
pub mod session;

use rules::engine::{RuleEngine, SessionSnapshot};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};

pub struct AppState {
    engine: Arc<Mutex<RuleEngine>>,
    stop: Mutex<Option<Sender<()>>>,
}

#[tauri::command]
fn start_session(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut stop_guard = state.stop.lock().unwrap();
    if stop_guard.is_some() {
        return Err("会话已在进行中".into());
    }
    // 重置引擎
    *state.engine.lock().unwrap() = RuleEngine::new();

    let models_dir: PathBuf = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("..")
        .join("models");
    // 开发模式下 resource_dir 指向 target/debug，models 在项目根：
    // 找不到时回退到 crate 根的上级
    let models_dir = if models_dir.join("silero_vad.onnx").exists() {
        models_dir
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models")
    };
    if !models_dir.join("silero_vad.onnx").exists() {
        return Err("模型文件缺失，请先运行 scripts/download-models.ps1".into());
    }

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    *stop_guard = Some(tx);
    let engine = Arc::clone(&state.engine);
    let app_clone = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = session::run_session(app_clone.clone(), rx, engine, models_dir) {
            let _ = app_clone.emit("session_error", e);
        }
    });
    Ok(())
}

#[tauri::command]
fn stop_session(state: State<AppState>) -> SessionSnapshot {
    if let Some(tx) = state.stop.lock().unwrap().take() {
        let _ = tx.send(());
    }
    state.engine.lock().unwrap().snapshot()
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> SessionSnapshot {
    state.engine.lock().unwrap().snapshot()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![start_session, stop_session, get_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build`
Expected: SUCCESS。若模板 `main.rs` 调用的是 `app_lib::run()`，确认 lib 名匹配（模板默认 crate 名 `app`，lib target 名 `app_lib`，保持模板原样即可）。

- [ ] **Step 3: Commit**

```powershell
cd D:\普通话
git add express-trainer/src-tauri
git commit -m "feat: tauri commands start/stop/get_snapshot wiring"
```

---

### Task 12: 前端三栏 UI + 事件订阅

**Files:**
- Create: `express-trainer/src/types.ts`
- Create: `express-trainer/src/highlight.ts`（填充词高亮纯函数）
- Create: `express-trainer/src/hooks/useSession.ts`
- Create: `express-trainer/src/components/SubtitleColumn.tsx`
- Create: `express-trainer/src/components/FeedbackColumn.tsx`
- Create: `express-trainer/src/components/StatsPanel.tsx`
- Modify: `express-trainer/src/App.tsx`（全文替换）
- Test: `express-trainer/src/highlight.test.ts`（vitest）

**Interfaces:**
- Consumes: Task 11 commands 与 `partial_transcript` / `sentence_final` / `analysis_update` / `session_error` 事件
- Produces: `highlightFillers(text: string, fillers: string[]): HighlightPart[]`；`useSession()` 返回 `{ running, partial, sentences, events, snapshot, fillerWords, start, stop }`

- [ ] **Step 1: 安装 vitest 并写高亮失败测试**

```powershell
cd D:\普通话\express-trainer
npm install -D vitest
```

`package.json` 的 `scripts` 加 `"test": "vitest run"`。

`src/highlight.test.ts`：

```ts
import { describe, it, expect } from "vitest";
import { highlightFillers } from "./highlight";

describe("highlightFillers", () => {
  it("marks filler occurrences", () => {
    const parts = highlightFillers("然后我想说然后就是", ["然后", "就是"]);
    expect(parts.filter((p) => p.isFiller)).toHaveLength(3);
    expect(parts.map((p) => p.text).join("")).toBe("然后我想说然后就是");
  });

  it("returns single part when no filler", () => {
    expect(highlightFillers("干净的一句话", ["然后"])).toEqual([
      { text: "干净的一句话", isFiller: false },
    ]);
  });
});
```

- [ ] **Step 2: 验证失败**

Run: `npm test`
Expected: FAIL — `highlight.ts` 不存在。

- [ ] **Step 3: 实现 `src/highlight.ts`**

```ts
export interface HighlightPart {
  text: string;
  isFiller: boolean;
}

export function highlightFillers(text: string, fillers: string[]): HighlightPart[] {
  if (fillers.length === 0) return [{ text, isFiller: false }];
  const pattern = new RegExp(`(${fillers.join("|")})`, "g");
  return text
    .split(pattern)
    .filter((s) => s.length > 0)
    .map((s) => ({ text: s, isFiller: fillers.includes(s) }));
}
```

- [ ] **Step 4: 验证通过**

Run: `npm test`
Expected: 2 个用例 PASS。

- [ ] **Step 5: 实现类型与会话 hook**

`src/types.ts`：

```ts
export interface Sentence {
  id: number;
  text: string;
  startMs: number;
  endMs: number;
}

export type FeedbackKind =
  | "fillerWord"
  | "wordPrecision"
  | "repetition"
  | "conclusionMissing"
  | "exampleMissing"
  | "emotion";

export interface FeedbackEvent {
  kind: FeedbackKind;
  sentenceId: number | null;
  message: string;
  payload: Record<string, unknown>;
}

export interface SessionSnapshot {
  sentenceCount: number;
  fillerCounts: [string, number][];
  fillerPerMinute: number;
  emotionCounts: [string, number][];
  durationMs: number;
}
```

`src/hooks/useSession.ts`：

```ts
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { FeedbackEvent, Sentence, SessionSnapshot } from "../types";

const DEFAULT_FILLERS = ["然后", "就是", "那个", "呃", "嗯", "其实", "比如说"];

export function useSession() {
  const [running, setRunning] = useState(false);
  const [partial, setPartial] = useState("");
  const [sentences, setSentences] = useState<Sentence[]>([]);
  const [events, setEvents] = useState<FeedbackEvent[]>([]);
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const unlisteners = useRef<(() => void)[]>([]);

  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<{ text: string }>("partial_transcript", (e) => setPartial(e.payload.text)),
      listen<Sentence>("sentence_final", (e) => {
        setPartial("");
        setSentences((prev) => [...prev, e.payload]);
      }),
      listen<{ events: FeedbackEvent[]; snapshot: SessionSnapshot }>(
        "analysis_update",
        (e) => {
          setEvents((prev) => [...prev, ...e.payload.events]);
          setSnapshot(e.payload.snapshot);
        }
      ),
      listen<string>("session_error", (e) => {
        setError(e.payload);
        setRunning(false);
      }),
    ];
    Promise.all(regs).then((fns) => {
      if (cancelled) fns.forEach((f) => f());
      else unlisteners.current = fns;
    });
    return () => {
      cancelled = true;
      unlisteners.current.forEach((f) => f());
      unlisteners.current = [];
    };
  }, []);

  const start = useCallback(async () => {
    setError(null);
    setSentences([]);
    setEvents([]);
    setSnapshot(null);
    setPartial("");
    try {
      await invoke("start_session");
      setRunning(true);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const stop = useCallback(async () => {
    const snap = await invoke<SessionSnapshot>("stop_session");
    setSnapshot(snap);
    setRunning(false);
    setPartial("");
  }, []);

  return { running, partial, sentences, events, snapshot, error, fillerWords: DEFAULT_FILLERS, start, stop };
}
```

- [ ] **Step 6: 实现三个栏位组件**

`src/components/SubtitleColumn.tsx`：

```tsx
import { useEffect, useRef } from "react";
import type { Sentence } from "../types";
import { highlightFillers } from "../highlight";

export function SubtitleColumn({
  sentences,
  partial,
  fillers,
}: {
  sentences: Sentence[];
  partial: string;
  fillers: string[];
}) {
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [sentences.length, partial]);

  return (
    <div className="flex h-full flex-col overflow-y-auto px-6 py-4">
      <div className="space-y-3 text-lg leading-relaxed">
        {sentences.map((s) => (
          <p key={s.id}>
            {highlightFillers(s.text, fillers).map((p, i) =>
              p.isFiller ? (
                <span key={i} className="rounded bg-red-500/20 px-0.5 font-medium text-red-600">
                  {p.text}
                </span>
              ) : (
                <span key={i}>{p.text}</span>
              )
            )}
          </p>
        ))}
        {partial && <p className="text-neutral-400">{partial}</p>}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
```

`src/components/FeedbackColumn.tsx`：

```tsx
import { useEffect, useRef } from "react";
import type { FeedbackEvent } from "../types";

const KIND_STYLE: Record<string, string> = {
  fillerWord: "border-red-300 text-red-700",
  wordPrecision: "border-amber-300 text-amber-700",
  repetition: "border-orange-300 text-orange-700",
  conclusionMissing: "border-blue-300 text-blue-700",
  exampleMissing: "border-teal-300 text-teal-700",
  emotion: "border-emerald-300 text-emerald-700",
};

export function FeedbackColumn({ events }: { events: FeedbackEvent[] }) {
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  return (
    <div className="h-full space-y-2 overflow-y-auto px-4 py-4">
      {events.map((e, i) => (
        <div key={i} className={`rounded-lg border-l-4 bg-white px-3 py-2 shadow-sm ${KIND_STYLE[e.kind] ?? ""}`}>
          <div className="text-sm font-medium">{e.message}</div>
          {e.kind === "wordPrecision" && Array.isArray(e.payload.alternatives) && (
            <div className="mt-1 text-sm text-neutral-600">
              {(e.payload.alternatives as string[]).join(" / ")}
            </div>
          )}
        </div>
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
```

`src/components/StatsPanel.tsx`：

```tsx
import type { SessionSnapshot } from "../types";

export function StatsPanel({ snapshot }: { snapshot: SessionSnapshot | null }) {
  if (!snapshot) {
    return <div className="px-4 py-4 text-sm text-neutral-400">开始说话后这里会显示统计</div>;
  }
  const seconds = Math.round(snapshot.durationMs / 1000);
  const mm = String(Math.floor(seconds / 60)).padStart(2, "0");
  const ss = String(seconds % 60).padStart(2, "0");
  return (
    <div className="space-y-4 px-4 py-4 text-sm">
      <div>
        <div className="text-neutral-500">时长</div>
        <div className="text-xl font-semibold">{mm}:{ss}</div>
      </div>
      <div>
        <div className="text-neutral-500">口头禅频率</div>
        <div className="text-xl font-semibold">{snapshot.fillerPerMinute} 次/分钟</div>
      </div>
      <div>
        <div className="mb-1 text-neutral-500">口头禅 Top</div>
        {snapshot.fillerCounts.slice(0, 5).map(([word, count]) => (
          <div key={word} className="flex justify-between">
            <span className="text-red-600">{word}</span>
            <span>{count}</span>
          </div>
        ))}
      </div>
      {snapshot.emotionCounts.length > 0 && (
        <div>
          <div className="mb-1 text-neutral-500">情感分布</div>
          {snapshot.emotionCounts.map(([cat, count]) => (
            <div key={cat} className="flex justify-between">
              <span>{cat}</span>
              <span>{count}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 7: 实现 `App.tsx`（全文替换）**

```tsx
import { useSession } from "./hooks/useSession";
import { SubtitleColumn } from "./components/SubtitleColumn";
import { FeedbackColumn } from "./components/FeedbackColumn";
import { StatsPanel } from "./components/StatsPanel";

export default function App() {
  const { running, partial, sentences, events, snapshot, error, fillerWords, start, stop } =
    useSession();

  return (
    <div className="flex h-screen flex-col bg-neutral-50 text-neutral-900">
      <header className="flex items-center justify-between border-b border-neutral-200 bg-white px-6 py-3">
        <h1 className="text-base font-semibold">表达训练系统</h1>
        <button
          onClick={running ? stop : start}
          className={`rounded-lg px-5 py-2 text-sm font-medium text-white ${
            running ? "bg-red-500 hover:bg-red-600" : "bg-neutral-900 hover:bg-neutral-700"
          }`}
        >
          {running ? "结束练习" : "开始练习"}
        </button>
      </header>
      {error && (
        <div className="border-b border-red-200 bg-red-50 px-6 py-2 text-sm text-red-700">
          {error}
        </div>
      )}
      <main className="grid min-h-0 flex-1 grid-cols-[260px_1fr_300px]">
        <aside className="border-r border-neutral-200 bg-white">
          <div className="border-b border-neutral-100 px-4 py-2 text-xs font-medium text-neutral-400">
            表达分析
          </div>
          <StatsPanel snapshot={snapshot} />
        </aside>
        <section className="min-w-0">
          <SubtitleColumn sentences={sentences} partial={partial} fillers={fillerWords} />
        </section>
        <aside className="border-l border-neutral-200 bg-neutral-50">
          <div className="border-b border-neutral-200 px-4 py-2 text-xs font-medium text-neutral-400">
            实时反馈
          </div>
          <FeedbackColumn events={events} />
        </aside>
      </main>
    </div>
  );
}
```

- [ ] **Step 8: 验证前端构建与测试**

```powershell
cd D:\普通话\express-trainer
npm test
npm run build
```

Expected: vitest PASS；`tsc && vite build` SUCCESS（若模板 build 脚本只是 `vite build` 且报 TS 错误，按报错修正类型）。

- [ ] **Step 9: Commit**

```powershell
cd D:\普通话
git add express-trainer
git commit -m "feat(ui): three-column realtime coaching interface"
```

---

### Task 13: 端到端冒烟验证 + README

**Files:**
- Create: `express-trainer/README.md`

**Interfaces:**
- Consumes: 全部前置任务

- [ ] **Step 1: 全量自动化测试**

```powershell
cd D:\普通话\express-trainer\src-tauri; cargo test
cd D:\普通话\express-trainer; npm test
```

Expected: 全 PASS。

- [ ] **Step 2: 启动应用做人工冒烟**

```powershell
cd D:\普通话\express-trainer
npm run tauri dev
```

人工检查清单：
1. 点「开始练习」→ 对着麦克风说一段话 → 中间栏出现灰字 partial，停顿后变成定稿句子
2. 刻意说「然后……然后……」→ 字幕中「然后」标红；左栏口头禅计数与频率上升；右栏出现口头禅卡片
3. 说「我想做一个很好的东西」→ 右栏出现「想/很好/东西」替换建议（全会话各只提示一次）
4. 把同一句话换个语气词说两遍 → 右栏出现「这句话已经说过一遍了」
5. 连续描述不给结论 → 第 5 句后出现结论提醒
6. 点「结束练习」→ 不报错，统计定格

发现的问题当场修复并补对应单测（规则类问题必须能写出 `cargo test` 复现用例再修）。

- [ ] **Step 3: 写 README**

`express-trainer/README.md`：

```markdown
# 表达训练系统

实时反馈的中文口头表达训练工具：说话时显示字幕、标红口头禅、给出词汇/结构提醒。

## 环境要求

- Node 18+、Rust 1.75+（Windows 需 MSVC Build Tools）

## 首次运行

1. `npm install`
2. `powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1`（下载 VAD + 流式 ASR 模型，约 600MB）
3. `npm run tauri dev`

## 测试

- Rust 规则引擎：`cd src-tauri && cargo test`
- 前端：`npm test`
```

- [ ] **Step 4: 最终 Commit**

```powershell
cd D:\普通话
git add express-trainer/README.md
git commit -m "docs: readme with setup instructions"
```

---

## Self-Review 记录

- Spec 覆盖：本计划覆盖 spec 的 M1（ASR 管线）与 M2（6 条规则 + 三栏 UI）。M3（报告/导出/设置）、M4（打包/下载管理器/历史会话）明确留给下一份计划，符合 spec 里程碑拆分。
- 占位符扫描：无 TBD/TODO；所有代码步骤含完整代码。
- 类型一致性：`Sentence`/`FeedbackEvent`/`SessionSnapshot` 字段在 Rust（camelCase serde）与 TS 类型间一一对应；`RuleEngine::ingest/snapshot/start`、`run_session`、`start_capture`、`resample_to_16k_mono` 签名在消费方任务中一致使用。
