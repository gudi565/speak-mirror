# SpeakMirror 词库 v2

`lexicon-v2.json` 是实时规则引擎（Rust）与报告层共用的核心数据文件（产品方案 §5.2）。本 README 说明数据来源、字段 schema 与 Rust 结构体对齐方式。

## 数据来源与授权（永久规避授权风险，见产品方案 §8）

| 部分 | 来源 | 许可 |
|---|---|---|
| 种子（情绪词 146 条、笼统→精准映射 25 组、填充词 25 个、犹豫词 20 个、程度梯度、画面化 10 组、犹豫→直接转换 8 组） | [expression-trainer](https://github.com/fxy2311-youyou/expression-trainer) 的 `data/emotion-lexicon.json`（本地快照：`seed-emotion-lexicon.json`） | MIT，可合法复用与修改 |
| 其余全部条目（情绪词补至 439 条、映射扩至 126 组、填充词分级 45 个、犹豫词 54 个、时间模糊词 25 条、画面化 23 组） | SpeakMirror 自建扩充（2026-08-30） | MIT，随本项目发布 |

说明：情绪词的七大类（乐/好/怒/哀/惧/恶/惊）只沿用大连理工情感词汇本体的**分类思想**；未使用其任何词表数据（该库明确禁商用）。种子原值保留原强度，扩充词条强度为人工校准。

## 规模统计

| 字段 | 规模 | 备注 |
|---|---|---|
| fillers | 高频 16 + 中频 29 = 45 | 原版 25 个，分级重排 |
| hedges | 54 | 原版 20 个 |
| vagueToPrecise | 126 组 | 原版 25 组；每组 2–6 个替代词 |
| emotionWords | 439 条 | 乐 66 / 好 77 / 怒 41 / 哀 100 / 惧 59 / 恶 54 / 惊 42 |
| timeVague | 25 条 | M2「时间模糊」新规则用 |
| imageryPairs | 23 组 | 原版 10 组 |
| intensityScale | 4 档 | 种子保留 |
| hedgeToDirectMap | 14 条 | 种子 8 条 + 扩充 6 条 |

`_meta.counts` 与实际条目数已程序校验一致；改动文件后请同步维护该字段（校验脚本见文末）。

## 字段 Schema 与语义

顶层结构（`_meta` 之外 9 个数据字段，前 6 个为 M2 必需）：

### 1. `fillers` — 填充词（口头禅），按频率分级

```json
{ "high": ["然后", "就是", ...], "medium": ["其实", "说白了", ...] }
```

- 类型：`{ high: string[], medium: string[] }`
- 语义：high = 最常爆发、优先计数与提醒的口头禅；medium = 次级，阈值更宽松（宁可漏报）。
- 用途：口头禅规则（计数 + 每分钟频率）；报告层「行为模式分析」的爆发语境。
- 匹配注意：**必须按词条长度降序（最长优先）**，否则「然后就是」会被拆成「然后」+「就是」重复计数；句尾语气词（哦/噢/呀）误报率高，建议仅在句首或独立成词时命中，或只给 medium 权重。

### 2. `hedges` — 犹豫弱化词（立场模糊）

类型：`string[]`。语义：削弱立场的词（可能/大概/差不多/还行…）。用途：单句堆叠 ≥2 个触发「立场模糊」规则（产品方案 §5.3）；报告层「回避与犹豫模式」。注意「应该/感觉」等词在正常句法中也高频出现，单独出现不触发、堆叠才触发。

### 3. `vagueToPrecise` — 笼统词 → 精准替代

类型：`Record<string, string[]>`（每组 2–6 个替代）。语义：键是口语高频模糊词，值是更精确/更书面/更有画面感的替代。用途：词汇精确度规则（命中计数）；报告「可替换词汇表」的数据源。替代词全部人工校订，无同义反复凑数。

### 4. `emotionWords` — 七大类情绪词（带强度 1–9）

```json
{ "乐": { "description": "…", "words": { "狂喜": 9, "开心": 5 } }, "好": {...}, "怒": {...}, "哀": {...}, "惧": {...}, "恶": {...}, "惊": {...} }
```

- 类型：`Record<类别, { description: string, words: Record<string, number> }>`，类别固定为乐/好/怒/哀/惧/恶/惊。
- 强度为 1–9 相对值：1–3 轻微，4–6 中等，7–8 强烈，9 极端。
- 用途：情感词规则（分类计数，见现有 `rules/engine.rs` 统计）；报告语气画像。跨类无重复词。

### 5. `timeVague` — 模糊时间词 → 具体化建议

类型：`Record<string, string>`。语义：键是模糊时间表达，值是一句"如何说具体"的建议（M2「时间模糊」新规则）。触发方式同 hedges：命中即温和提示一次。

### 6. `imageryPairs` — 抽象 → 具象对子

类型：`Record<string, string[]>`。语义：抽象表达（很累/很忙）对应有画面感的具象说法。**只给报告层用作改写素材**，本地实时层不做整句改写（§5.3 纪律）。

### 7–9. 附加字段（种子保留）

- `intensityScale`：程度词 4 档梯度（弱/中/强/极），供报告分析"程度副词堆叠"用。
- `hedgeToDirectMap`：犹豫句式 → 直接句式转换建议（含 `description` 键，实体条目 14 条），报告层「逐句改写」参考。

## Rust 结构体对齐（serde）

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LexiconV2 {
    #[serde(rename = "_meta")]
    pub meta: LexiconMeta,
    pub fillers: Fillers,
    pub hedges: Vec<String>,
    #[serde(rename = "vagueToPrecise")]
    pub vague_to_precise: HashMap<String, Vec<String>>,
    #[serde(rename = "emotionWords")]
    pub emotion_words: HashMap<EmotionCategory, EmotionGroup>,
    #[serde(rename = "timeVague")]
    pub time_vague: HashMap<String, String>,
    #[serde(rename = "imageryPairs")]
    pub imagery_pairs: HashMap<String, Vec<String>>,
    #[serde(rename = "intensityScale")]
    pub intensity_scale: IntensityScale,
    #[serde(rename = "hedgeToDirectMap")]
    pub hedge_to_direct_map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fillers { pub high: Vec<String>, pub medium: Vec<String> }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmotionCategory { 乐, 好, 怒, 哀, 惧, 恶, 惊 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmotionGroup { pub description: String, pub words: HashMap<String, u8> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntensityScale {
    pub description: String,
    #[serde(rename = "弱")] pub weak: Vec<String>,
    #[serde(rename = "中")] pub medium: Vec<String>,
    #[serde(rename = "强")] pub strong: Vec<String>,
    #[serde(rename = "极")] pub extreme: Vec<String>,
}

// LexiconMeta：name/version/created/description/license 均为 String，
// sources: Vec<LexiconSource{ name, url, license, usage }>，
// notes: Vec<String>，counts 可直接用 serde_json::Value（仅自校验用）。
// 加载：include_str!("../../docs/lexicon/lexicon-v2.json") 编译期内嵌，启动时 parse 一次入 OnceLock。
```

字段类型速查：`fillers` = 两档字符串数组；`hedges` = 字符串数组；`vagueToPrecise` / `imageryPairs` = `HashMap<String, Vec<String>>`；`emotionWords` = `HashMap<类别, {description, words: HashMap<String, u8>}>`；`timeVague` / `hedgeToDirectMap` = `HashMap<String, String>`（注意这两个字段里各有一个 `description` 元数据键，规则侧遍历时跳过或用独立结构体剥离）。

## 维护流程（词库自生长机制，产品方案 §5.2）

1. 报告层发现高频出现但**无映射**的模糊词 → 自动进「候选扩充清单」（M3 落盘）。
2. 人工审核候选词：确认替代词确实更精确/书面/有画面感，才并入 `vagueToPrecise`（重质量不凑数）。
3. 用户自定义口头禅是**运行时配置**，不写入本文件；本文件只做内置基线。
4. 任何改动后跑一遍校验（JSON 合法 + 无重复键 + 计数一致）：

```sh
python -X utf8 -c "import json;d=json.load(open(r'docs/lexicon/lexicon-v2.json',encoding='utf-8'));print(sum(len(c['words']) for c in d['emotionWords'].values()), len(d['vagueToPrecise']), len(d['hedges']))"
```
