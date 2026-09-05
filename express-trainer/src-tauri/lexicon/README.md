# SpeakMirror 词库目录（src-tauri/lexicon）

本目录存放规则引擎的编译期内嵌词库（`include_str!`，启动时解析一次入缓存，
损坏即 panic——词库是产品地基）。设计文档与完整 schema 说明在
`docs/lexicon/README.md`（docs 只读，本目录为生效副本）。

| 文件 | 内容 | 授权 |
|---|---|---|
| `lexicon-v2.json` | 中文词库 v2：填充词分级 45、犹豫弱化词 54、笼统→精准 126 组、七大类情绪词 439、时间模糊 25、画面化 23 组。种子来自 MIT 授权的 expression-trainer，其余自建扩充 | MIT（来源记录见文件 `_meta.sources` 与 `docs/lexicon/README.md`） |
| `lexicon-en.json` | 英文词库 v1：填充词分级 36（high 17 / medium 19）、犹豫弱化词 27、笼统→精准 76 组 | **全部自建**（2026-09-05 人工编订），MIT 随本项目发布，未使用任何第三方词表数据 |
| `pinyin-tones.tsv` | 声调检查（tone.rs）用的拼音-声调词典 | 自建 |

## 英文词库说明（lexicon-en.json）

- **英文侧只覆盖语言无关/可移植的三类数据**（fillers / hedges / vagueToPrecise）。
  情绪词、时间模糊、画面感、金句启发式是中文特有规则，英文句子直接跳过，
  英文侧不强做对应数据。
- **匹配契约**：纯 ASCII 词条（含撇号如 `don't`、含空格短语如 `you know`）
  按词边界匹配（等效 `\b` 语义，手写判断、大小写不敏感），多词短语整体匹配、
  逐词边界；最长优先不变（`sort of` 先于 `so`）。见 `rules/lexicon.rs` 的
  `WordMatcher`。
- **分级策略**：容易在正常句法里误报的词（`so`、`well`、`right` 等）一律放
  medium 档——只有词频达到阈值才逐次提醒；`right?` 带问号，只命中句尾疑问
  语气词用法，不命中形容词 right。
- **跨字段重叠是有意设计**（`kind of`/`sort of` 同时是 filler 与 hedge；
  `a bit` 同时是 hedge 与 vagueToPrecise 键），与中文词库口径一致：不同规则
  各取所需。
- **_meta.counts 维护纪律**：改动文件后同步维护 `_meta.counts`，
  `rules/lexicon.rs` 的测试会程序校验条目数与之一致。

## 与代码的关系

- 解析与缓存：`src-tauri/src/rules/lexicon.rs`（`builtin_lexicon()` 中文 /
  `builtin_lexicon_en()` 英文）。
- 语言路由：`src-tauri/src/rules/lang.rs`（句子级 CJK 占比 ≥30% 走中文，
  否则走英文；混合句两套都跑）。
- 用户词库（`appdata/user-lexicon.json` 的 vagueToPrecise 条目）只合并进中文
  词库（`merge_user_lexicon`）；设置里的自定义口头禅按文字自动归入对应语言的
  口头禅规则（含 CJK 进中文表、纯 ASCII 进英文表）。英文词库当前为纯内置，
  英文侧用户自增长留后续。
