//! 应用设置：持久化到 appdata 的 settings.json（由 tauri-plugin-store 管理），
//! 前端通过 @tauri-apps/plugin-store 写入同一 store 的 "settings" 键，Rust 侧只读。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

pub const STORE_FILE: &str = "settings.json";
pub const STORE_KEY: &str = "settings";

/// 后端预设：各自带默认 baseURL 与默认 model 名。
/// 与前端 src/lib/settings.ts 中的 PRESETS 保持同步。
pub struct Preset {
    pub base_url: &'static str,
    pub model: &'static str,
}

pub const BACKENDS: &[&str] = &["deepseek", "openai", "groq", "ollama", "custom"];

/// 设置持久化的场景白名单（与前端 types.ts Scenario、report.rs SCENARIOS 对应）：
/// 自由练习 / 面试回答 / 口播视频 / 工作汇报。
/// mockInterview 只在模拟面试流程内部经 generate_report 使用，不进设置与
/// 普通场景下拉，因此不在本白名单里（normalized 会把它回落为 free）。
pub const SCENARIOS: &[&str] = &["free", "interview", "vlog", "workreport"];

pub fn preset_for(backend: &str) -> Option<Preset> {
    match backend {
        "deepseek" => Some(Preset {
            base_url: "https://api.deepseek.com/v1",
            model: "deepseek-chat",
        }),
        "openai" => Some(Preset {
            base_url: "https://api.openai.com/v1",
            model: "gpt-4o-mini",
        }),
        "groq" => Some(Preset {
            base_url: "https://api.groq.com/openai/v1",
            model: "llama-3.3-70b-versatile",
        }),
        "ollama" => Some(Preset {
            base_url: "http://localhost:11434/v1",
            model: "qwen2.5:7b",
        }),
        // custom 无预设，base_url/model 必须用户自己填
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// deepseek / openai / groq / ollama / custom
    pub ai_backend: String,
    pub api_key: String,
    /// 留空 = 使用预设默认；custom / 覆盖预设时填写
    pub base_url: String,
    /// 留空 = 使用预设默认
    pub model_name: String,
    /// Obsidian vault 根目录，留空表示未配置（导出时退回保存对话框）
    pub obsidian_vault_path: String,
    /// free / interview / vlog / workreport
    pub scenario: String,
    /// 每条规则单独开关（键 = 规则名，缺省视为开启；规则名见 rules::engine::ALL_RULES）
    #[serde(default)]
    pub rule_enabled: std::collections::HashMap<String, bool>,
    /// 口头禅高频阈值（次/分钟）：中频口头禅词的词频达到该值后才逐次提醒
    #[serde(default = "default_filler_high_threshold")]
    pub filler_high_threshold: f64,
    /// 用户自定义填充词（中英文逗号分隔），合并进口头禅规则，视同高频词
    #[serde(default)]
    pub custom_fillers: String,
    /// AI 周期快评开关（默认关闭，产品方案 §5.3 / §8：宁可漏报，不可刷屏）
    #[serde(default)]
    pub realtime_checkin_enabled: bool,
    /// AI 周期快评频率（秒）
    #[serde(default = "default_checkin_interval_sec")]
    pub realtime_checkin_interval_sec: u32,
    /// 口头禅目标频率（次/分钟，M3 成长档案）：None = 未设目标
    #[serde(default)]
    pub filler_goal_per_min: Option<f64>,
    /// 首启引导是否已完成（M4）：false = 启动时进入三步向导
    #[serde(default)]
    pub onboarding_done: bool,
    /// 会话录音（结束后逐句回放用；音频仅保存在本机 appdata，默认开）
    #[serde(default = "default_record_audio")]
    pub record_audio: bool,
    /// 识别热词（逗号/换行分隔；空 = 不启用）。传给 sherpa-onnx 的
    /// hotwords_file（仅 transducer 系模型在 modified_beam_search 下生效）
    #[serde(default)]
    pub hotwords: String,
    /// 热词权重：None = 默认 1.5（常用区间 1.5–2.5）
    #[serde(default)]
    pub hotwords_score: Option<f64>,
    /// 识别纠错映射（每行一条「错->对」，如 深seek->DeepSeek）：
    /// 终稿句进规则引擎前做文本替换（当前流式 Paraformer 模型不消费
    /// 解码级热词，这层保证热词类需求对现有模型可感知）
    #[serde(default)]
    pub asr_corrections: String,
    /// 高精度终稿（双引擎，默认开）：SenseVoice 模型就绪时句子定稿优先走
    /// 离线引擎（自带标点与 ITN）；缺失/关闭回退流式终稿
    #[serde(default = "default_precision_finals")]
    pub precision_finals: bool,
    /// 断句灵敏度：standard（Silero 阈值 0.5，默认）/ high（0.35，
    /// 远距离或小声录音时轻尾音不易被切掉）
    #[serde(default)]
    pub vad_sensitivity: String,
    /// 录音增强（自动增益，默认开）：文件模式解码重采样后检测整段电平
    /// （帧 RMS p95 < 0.08 视为过静）则线性放大（上限 8×，钳位 ±1），
    /// 改善手机远距离录音的断句与识别。麦克风实时路径不启用。
    /// 与断句灵敏度「高」互补：过静录音建议两者都开。
    #[serde(default = "default_enhance_audio")]
    pub enhance_audio: bool,
    /// 显示实时识别预览（默认开）：关闭后中栏仅显示每句定稿句，观感更稳。
    /// 纯前端展示开关，Rust 会话管线不消费，仅随 store 持久化保持两侧结构一致
    #[serde(default = "default_show_live_preview")]
    pub show_live_preview: bool,
    /// 声调偏差检查（默认开）：会话停止且录音存在时，后台线程对录音做
    /// 纯本机的基音轨迹 + 词典声调比对（tone.rs），结果经 tone_update 事件
    /// 推给前端「声调提示」面板。不联网、不上传录音
    #[serde(default = "default_tone_check")]
    pub tone_check: bool,
    /// AI 智能层激活横幅已「暂不提醒」（true = 主界面不再显示横幅）：
    /// 前端写 store、Rust 侧只读，保持两侧 Settings 结构一致；
    /// 配置远端后端后横幅条件自然不再成立
    #[serde(default)]
    pub ai_nudge_dismissed: bool,
}

pub fn default_filler_high_threshold() -> f64 {
    3.0
}

pub fn default_checkin_interval_sec() -> u32 {
    45
}

pub fn default_record_audio() -> bool {
    true
}

pub fn default_precision_finals() -> bool {
    true
}

pub fn default_enhance_audio() -> bool {
    true
}

pub fn default_show_live_preview() -> bool {
    true
}

pub fn default_tone_check() -> bool {
    true
}

/// 断句灵敏度合法取值（与前端 types.ts 的 VadSensitivity 对应）
pub const VAD_SENSITIVITIES: &[&str] = &["standard", "high"];

/// 断句灵敏度 → Silero VAD 阈值：标准 0.5 / 高灵敏度 0.35
/// （高灵敏度更容易保住轻尾音与远场小声，代价是嘈杂环境下可能多切）
pub fn vad_threshold(sensitivity: &str) -> f32 {
    if sensitivity == "high" {
        0.35
    } else {
        0.5
    }
}

/// 热词默认权重（sherpa-onnx 常用区间 1.5–2.5，取最保守端）
pub const DEFAULT_HOTWORDS_SCORE: f64 = 1.5;

/// 自定义填充词解析：容忍中英文逗号 / 顿号，去空白、去空词、去重
pub fn parse_custom_fillers(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', '，', '、']) {
        let w = part.trim();
        if !w.is_empty() && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// 热词解析：逗号 / 顿号 / 换行分隔，去空白、去空词、去重（保持输入顺序）
pub fn parse_hotwords(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', '，', '、', '\n', '\r', ';', '；']) {
        let w = part.trim();
        if !w.is_empty() && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// 纠错映射一条：错误写法 → 正确写法
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub from: String,
    pub to: String,
}

/// 纠错映射解析：每行一条「错->对」，分隔符支持 `->` / `→` / `=`；
/// 无分隔符、空侧、空行的条目跳过；from 重复时保留第一条。
pub fn parse_correction_map(raw: &str) -> Vec<Correction> {
    let mut out: Vec<Correction> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (from, to) = if let Some((a, b)) = line.split_once("->") {
            (a, b)
        } else if let Some((a, b)) = line.split_once("→") {
            (a, b)
        } else if let Some((a, b)) = line.split_once('=') {
            (a, b)
        } else {
            continue;
        };
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            continue;
        }
        if !out.iter().any(|c| c.from == from) {
            out.push(Correction { from: from.to_string(), to: to.to_string() });
        }
    }
    out
}

/// 终稿句纠错：按映射逐条全量替换（from 较长者优先，避免「深seek」被
/// 「深」的短映射抢先吃掉）；仅做一轮直接替换，不递归。
pub fn apply_corrections(text: &str, map: &[Correction]) -> String {
    if map.is_empty() || text.is_empty() {
        return text.to_string();
    }
    let mut ordered: Vec<&Correction> = map.iter().collect();
    ordered.sort_by(|a, b| b.from.len().cmp(&a.from.len()));
    let mut out = text.to_string();
    for c in ordered {
        if out.contains(&c.from) {
            out = out.replace(&c.from, &c.to);
        }
    }
    out
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ai_backend: "deepseek".into(),
            api_key: String::new(),
            base_url: String::new(),
            model_name: String::new(),
            obsidian_vault_path: String::new(),
            scenario: "free".into(),
            rule_enabled: std::collections::HashMap::new(),
            filler_high_threshold: default_filler_high_threshold(),
            custom_fillers: String::new(),
            realtime_checkin_enabled: false,
            realtime_checkin_interval_sec: default_checkin_interval_sec(),
            filler_goal_per_min: None,
            onboarding_done: false,
            record_audio: default_record_audio(),
            hotwords: String::new(),
            hotwords_score: None,
            asr_corrections: String::new(),
            precision_finals: default_precision_finals(),
            vad_sensitivity: "standard".into(),
            enhance_audio: default_enhance_audio(),
            show_live_preview: default_show_live_preview(),
            tone_check: default_tone_check(),
            ai_nudge_dismissed: false,
        }
    }
}

impl Settings {
    /// 规则是否启用：开关缺省 = 开（unknown 键也按开处理，向前兼容）
    pub fn rule_enabled(&self, name: &str) -> bool {
        !self.rule_enabled.contains_key(name) || self.rule_enabled[name]
    }

    /// 归一化非法取值：未知的 backend / scenario 回落到默认值；阈值与快评频率夹到合法区间
    pub fn normalized(self) -> Self {
        let mut s = self;
        if !BACKENDS.contains(&s.ai_backend.as_str()) {
            s.ai_backend = "deepseek".into();
        }
        if !SCENARIOS.contains(&s.scenario.as_str()) {
            s.scenario = "free".into();
        }
        if !s.filler_high_threshold.is_finite() || s.filler_high_threshold < 0.0 {
            s.filler_high_threshold = default_filler_high_threshold();
        }
        s.filler_high_threshold = s.filler_high_threshold.clamp(0.0, 60.0);
        if s.realtime_checkin_interval_sec == 0 || s.realtime_checkin_interval_sec > 600 {
            s.realtime_checkin_interval_sec = default_checkin_interval_sec();
        }
        s.realtime_checkin_interval_sec = s.realtime_checkin_interval_sec.clamp(15, 600);
        // 口头禅目标：非法值（非有限/负数）视为未设目标；上限与阈值一致夹到 60
        if let Some(g) = s.filler_goal_per_min {
            if !g.is_finite() || g < 0.0 {
                s.filler_goal_per_min = None;
            } else {
                s.filler_goal_per_min = Some(g.clamp(0.0, 60.0));
            }
        }
        // 热词权重：非法值（非有限/非正）视为未设（用默认）；合法值夹到 [0.5, 5.0]
        if let Some(sc) = s.hotwords_score {
            if !sc.is_finite() || sc <= 0.0 {
                s.hotwords_score = None;
            } else {
                s.hotwords_score = Some(sc.clamp(0.5, 5.0));
            }
        }
        // 断句灵敏度：白名单外回落标准
        if !VAD_SENSITIVITIES.contains(&s.vad_sensitivity.as_str()) {
            s.vad_sensitivity = "standard".into();
        }
        // 录音增强：布尔开关无非法取值需要归一（serde default 已兜底）
        s
    }

    /// 生效的 (base_url, model)：用户留空的字段回落到后端预设默认值。
    /// custom 且未填 base_url 时返回 None。
    pub fn resolve_endpoint(&self) -> Option<(String, String)> {
        let preset = preset_for(&self.ai_backend);
        let base_url = if self.base_url.trim().is_empty() {
            preset.as_ref().map(|p| p.base_url.to_string())?
        } else {
            self.base_url.trim().to_string()
        };
        let model = if self.model_name.trim().is_empty() {
            preset.as_ref().map(|p| p.model.to_string())?
        } else {
            self.model_name.trim().to_string()
        };
        Some((base_url, model))
    }

    /// 是否具备发起远端请求的条件（Ollama 本地服务不需要 Key）
    pub fn can_call_remote(&self) -> bool {
        match self.ai_backend.as_str() {
            "ollama" => true,
            _ => !self.api_key.trim().is_empty(),
        }
    }
}

/// 从 tauri-plugin-store 读取设置；文件不存在或解析失败时返回默认值。
/// API Key 的生效值由系统凭据管理器（keyring）决定：
/// - 凭据里有 → 用凭据的，并顺手清掉 store 里的遗留明文；
/// - 凭据里没有但 store 有旧明文 → 自动迁移进凭据（成功后清明文；失败保留降级）。
pub fn load(app: &AppHandle) -> Settings {
    let store = app.store(STORE_FILE).ok();
    let mut s: Settings = store
        .as_ref()
        .and_then(|st| st.get(STORE_KEY))
        .and_then(|v| serde_json::from_value::<Settings>(v).ok())
        .unwrap_or_default();
    match crate::secrets::resolve_key(&crate::secrets::SystemKeyring::ai_api_key(), &s.api_key) {
        crate::secrets::ResolvedKey::Keyring { key, clear_plain } => {
            if clear_plain {
                clear_plain_key(store.as_deref(), &mut s);
            }
            s.api_key = key;
        }
        crate::secrets::ResolvedKey::Migrated { key } => {
            clear_plain_key(store.as_deref(), &mut s);
            s.api_key = key;
        }
        crate::secrets::ResolvedKey::Degraded { key } => {
            // 凭据管理器不可用：明文保留在 store 里继续可用（降级模式）
            s.api_key = key;
        }
        crate::secrets::ResolvedKey::None => {
            s.api_key = String::new();
        }
    }
    s.normalized()
}

/// 把 store 里的 apiKey 字段清空（迁移成功 / 凭据为准后调用；失败静默，下次再试）
fn clear_plain_key(store: Option<&tauri_plugin_store::Store<tauri::Wry>>, s: &mut Settings) {
    let Some(store) = store else { return };
    s.api_key = String::new();
    if let Ok(v) = serde_json::to_value(&*s) {
        let _ = store.set(STORE_KEY, v);
        let _ = store.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_deepseek_free_scenario() {
        let s = Settings::default();
        assert_eq!(s.ai_backend, "deepseek");
        assert_eq!(s.scenario, "free");
        assert_eq!(s.api_key, "");
    }

    #[test]
    fn presets_cover_all_non_custom_backends() {
        for b in BACKENDS {
            if *b == "custom" {
                assert!(preset_for(b).is_none());
            } else {
                let p = preset_for(b).unwrap();
                assert!(p.base_url.starts_with("http"));
                assert!(!p.model.is_empty());
            }
        }
    }

    #[test]
    fn resolve_falls_back_to_preset_when_blank() {
        let s = Settings { base_url: "  ".into(), model_name: String::new(), ..Default::default() };
        let (base, model) = s.resolve_endpoint().unwrap();
        assert_eq!(base, "https://api.deepseek.com/v1");
        assert_eq!(model, "deepseek-chat");
    }

    #[test]
    fn resolve_honors_user_override() {
        let s = Settings {
            base_url: "https://my-proxy.example.com/v1".into(),
            model_name: "my-model".into(),
            ..Default::default()
        };
        let (base, model) = s.resolve_endpoint().unwrap();
        assert_eq!(base, "https://my-proxy.example.com/v1");
        assert_eq!(model, "my-model");
    }

    #[test]
    fn resolve_fails_for_custom_without_base_url() {
        let s = Settings { ai_backend: "custom".into(), ..Default::default() };
        assert!(s.resolve_endpoint().is_none());
    }

    #[test]
    fn normalized_fixes_invalid_values() {
        let s = Settings { ai_backend: "bogus".into(), scenario: "xxx".into(), ..Default::default() };
        let s = s.normalized();
        assert_eq!(s.ai_backend, "deepseek");
        assert_eq!(s.scenario, "free");
    }

    #[test]
    fn normalized_keeps_all_four_scenarios() {
        // 场景枚举扩展：free / interview / vlog / workreport 全部合法
        for sc in SCENARIOS {
            let s = Settings { scenario: sc.to_string(), ..Default::default() }.normalized();
            assert_eq!(&s.scenario, sc);
        }
    }

    #[test]
    fn ollama_needs_no_key_others_do() {
        let no_key = Settings::default();
        assert!(!no_key.can_call_remote());
        let ollama = Settings { ai_backend: "ollama".into(), ..Default::default() };
        assert!(ollama.can_call_remote());
    }

    #[test]
    fn settings_roundtrip_camel_case() {
        let s = Settings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["aiBackend"], "deepseek");
        assert!(v.get("obsidianVaultPath").is_some());
        assert!(v.get("ruleEnabled").is_some());
        assert_eq!(v["fillerHighThreshold"], 3.0);
        assert_eq!(v["realtimeCheckinEnabled"], false);
        assert_eq!(v["realtimeCheckinIntervalSec"], 45);
        let back: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn legacy_settings_json_loads_with_new_defaults() {
        // M1 时代的 settings.json 没有 M2 字段：serde 默认值补齐
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "interview" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert_eq!(s.ai_backend, "openai");
        assert_eq!(s.filler_high_threshold, 3.0);
        assert_eq!(s.realtime_checkin_enabled, false);
        assert_eq!(s.realtime_checkin_interval_sec, 45);
        assert!(s.rule_enabled.is_empty());
    }

    #[test]
    fn rule_enabled_defaults_to_true_and_respects_switch() {
        let mut s = Settings::default();
        assert!(s.rule_enabled("filler_words"));
        assert!(s.rule_enabled("unknown_rule"));
        s.rule_enabled.insert("hedge".into(), false);
        assert!(!s.rule_enabled("hedge"));
        assert!(s.rule_enabled("filler_words"));
    }

    #[test]
    fn parse_custom_fillers_splits_and_dedupes() {
        assert_eq!(
            parse_custom_fillers("老铁, 绝绝子，就是说、 老铁 "),
            vec!["老铁".to_string(), "绝绝子".to_string(), "就是说".to_string()]
        );
        assert!(parse_custom_fillers("  ，,、 ").is_empty());
        assert!(parse_custom_fillers("").is_empty());
    }

    #[test]
    fn normalized_clamps_threshold_and_interval() {
        let s = Settings { filler_high_threshold: f64::NAN, realtime_checkin_interval_sec: 5, ..Default::default() };
        let s = s.normalized();
        assert_eq!(s.filler_high_threshold, 3.0);
        assert_eq!(s.realtime_checkin_interval_sec, 15);
        let s = Settings { filler_high_threshold: 999.0, realtime_checkin_interval_sec: 9999, ..Default::default() };
        let s = s.normalized();
        assert_eq!(s.filler_high_threshold, 60.0);
        assert_eq!(s.realtime_checkin_interval_sec, 45);
    }

    #[test]
    fn filler_goal_normalized_and_legacy_default() {
        // 旧 settings.json 无该字段 → None
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "interview" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert_eq!(s.filler_goal_per_min, None);
        // 合法目标保留；非法回落 None；超界夹到 60
        let s = Settings { filler_goal_per_min: Some(2.5), ..Default::default() }.normalized();
        assert_eq!(s.filler_goal_per_min, Some(2.5));
        let s = Settings { filler_goal_per_min: Some(-1.0), ..Default::default() }.normalized();
        assert_eq!(s.filler_goal_per_min, None);
        let s = Settings { filler_goal_per_min: Some(f64::NAN), ..Default::default() }.normalized();
        assert_eq!(s.filler_goal_per_min, None);
        let s = Settings { filler_goal_per_min: Some(999.0), ..Default::default() }.normalized();
        assert_eq!(s.filler_goal_per_min, Some(60.0));
        // 序列化 camelCase
        let v = serde_json::to_value(Settings { filler_goal_per_min: Some(2.5), ..Default::default() }).unwrap();
        assert_eq!(v["fillerGoalPerMin"], 2.5);
    }

    #[test]
    fn onboarding_done_defaults_false_and_serializes_camel() {
        // M4 之前的 settings.json 无该字段 → false（启动进首启向导）
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(!s.onboarding_done);
        // 序列化键名 camelCase，与前端 store 对齐
        let v = serde_json::to_value(Settings { onboarding_done: true, ..Default::default() }).unwrap();
        assert_eq!(v["onboardingDone"], true);
        let back: Settings = serde_json::from_value(v).unwrap();
        assert!(back.onboarding_done);
        assert!(!Settings::default().onboarding_done);
    }

    #[test]
    fn record_audio_defaults_true_and_legacy_uses_default() {
        // 默认开（会话录音回放）；旧 settings.json 无该字段 → 默认值
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.record_audio);
        assert!(Settings::default().record_audio);
        // 序列化 camelCase + round-trip 保留显式关闭
        let v = serde_json::to_value(Settings { record_audio: false, ..Default::default() }).unwrap();
        assert_eq!(v["recordAudio"], false);
        assert!(!serde_json::from_value::<Settings>(v).unwrap().record_audio);
    }

    #[test]
    fn parse_hotwords_splits_across_commas_and_lines() {
        assert_eq!(
            parse_hotwords("DeepSeek, 米糕\n大模型，、小米；\r\nDeepSeek"),
            vec!["DeepSeek".to_string(), "米糕".to_string(), "大模型".to_string(), "小米".to_string()]
        );
        assert!(parse_hotwords("  ，,、 \n\r ").is_empty());
        assert!(parse_hotwords("").is_empty());
    }

    #[test]
    fn hotwords_score_default_none_and_normalized() {
        // 默认 None（= 代码里的 DEFAULT_HOTWORDS_SCORE）
        assert_eq!(Settings::default().hotwords_score, None);
        assert_eq!(DEFAULT_HOTWORDS_SCORE, 1.5);
        // 合法值保留；非法值（非有限/非正）→ None；超界夹到 [0.5, 5.0]
        let s = Settings { hotwords_score: Some(2.5), ..Default::default() }.normalized();
        assert_eq!(s.hotwords_score, Some(2.5));
        let s = Settings { hotwords_score: Some(0.1), ..Default::default() }.normalized();
        assert_eq!(s.hotwords_score, Some(0.5));
        let s = Settings { hotwords_score: Some(99.0), ..Default::default() }.normalized();
        assert_eq!(s.hotwords_score, Some(5.0));
        let s = Settings { hotwords_score: Some(-1.0), ..Default::default() }.normalized();
        assert_eq!(s.hotwords_score, None);
        let s = Settings { hotwords_score: Some(f64::NAN), ..Default::default() }.normalized();
        assert_eq!(s.hotwords_score, None);
        // 序列化 camelCase
        let v = serde_json::to_value(Settings { hotwords_score: Some(2.0), ..Default::default() }).unwrap();
        assert_eq!(v["hotwordsScore"], 2.0);
    }

    #[test]
    fn parse_correction_map_accepts_arrow_equals_and_skips_bad_lines() {
        let raw = "深seek->DeepSeek\n大模 型→大模型\nai = AI\n没有分隔符\n->只有对\n只有错->\n深seek->另一个";
        let map = parse_correction_map(raw);
        assert_eq!(
            map,
            vec![
                Correction { from: "深seek".into(), to: "DeepSeek".into() },
                Correction { from: "大模 型".into(), to: "大模型".into() },
                Correction { from: "ai".into(), to: "AI".into() },
            ]
        );
        assert!(parse_correction_map("").is_empty());
        assert!(parse_correction_map("\n  \n").is_empty());
    }

    #[test]
    fn apply_corrections_replaces_longest_first_and_keeps_plain_text() {
        let map = parse_correction_map("深seek->DeepSeek\n深->神");
        // 长映射优先：不能先被「深->神」吃掉
        assert_eq!(apply_corrections("我常用深seek做开发", &map), "我常用DeepSeek做开发");
        // 没有命中长映射时短映射仍生效
        assert_eq!(apply_corrections("这段话很深奥", &map), "这段话很神奥");
        // 「深->神」单独生效
        assert_eq!(apply_corrections("深思考", &parse_correction_map("深->神")), "神思考");
        // 空映射 / 空文本直通
        assert_eq!(apply_corrections("原文", &[]), "原文");
        assert_eq!(apply_corrections("", &map), "");
    }

    #[test]
    fn precision_finals_defaults_true_and_legacy_uses_default() {
        // 旧 settings.json 无该字段 → 默认开（双引擎是默认体验）
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.precision_finals);
        assert!(Settings::default().precision_finals);
        // 序列化 camelCase + round-trip 保留显式关闭
        let v = serde_json::to_value(Settings { precision_finals: false, ..Default::default() }).unwrap();
        assert_eq!(v["precisionFinals"], false);
        assert!(!serde_json::from_value::<Settings>(v).unwrap().precision_finals);
    }

    #[test]
    fn vad_sensitivity_whitelist_and_threshold_mapping() {
        // 默认 standard；旧字段缺失 → standard；白名单外回落 standard
        assert_eq!(Settings::default().vad_sensitivity, "standard");
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert_eq!(s.vad_sensitivity, "");
        assert_eq!(s.normalized().vad_sensitivity, "standard");
        let s = Settings { vad_sensitivity: "high".into(), ..Default::default() }.normalized();
        assert_eq!(s.vad_sensitivity, "high");
        let s = Settings { vad_sensitivity: "loud".into(), ..Default::default() }.normalized();
        assert_eq!(s.vad_sensitivity, "standard");
        // 灵敏度 → Silero 阈值：标准 0.5 / 高 0.35；未知值按标准
        assert_eq!(vad_threshold("standard"), 0.5);
        assert_eq!(vad_threshold("high"), 0.35);
        assert_eq!(vad_threshold(""), 0.5);
        assert_eq!(vad_threshold("bogus"), 0.5);
        // 序列化 camelCase
        let v = serde_json::to_value(Settings { vad_sensitivity: "high".into(), ..Default::default() }).unwrap();
        assert_eq!(v["vadSensitivity"], "high");
    }

    #[test]
    fn enhance_audio_defaults_true_and_legacy_uses_default() {
        // 默认开（过静手机录音是高频痛点）；旧 settings.json 无该字段 → 默认值
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.enhance_audio);
        assert!(Settings::default().enhance_audio);
        // 序列化 camelCase + round-trip 保留显式关闭
        let v = serde_json::to_value(Settings { enhance_audio: false, ..Default::default() }).unwrap();
        assert_eq!(v["enhanceAudio"], false);
        assert!(!serde_json::from_value::<Settings>(v).unwrap().enhance_audio);
    }

    #[test]
    fn show_live_preview_defaults_true_and_legacy_uses_default() {
        // 默认开（定稿为主、预览为辅）；旧 settings.json 无该字段 → 默认值
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.show_live_preview);
        assert!(Settings::default().show_live_preview);
        // 序列化 camelCase + round-trip 保留显式关闭
        let v = serde_json::to_value(Settings { show_live_preview: false, ..Default::default() }).unwrap();
        assert_eq!(v["showLivePreview"], false);
        assert!(!serde_json::from_value::<Settings>(v).unwrap().show_live_preview);
    }

    #[test]
    fn tone_check_defaults_true_and_legacy_uses_default() {
        // 声调偏差检查默认开；旧 settings.json 无该字段 → 默认值
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(s.tone_check);
        assert!(Settings::default().tone_check);
        assert_eq!(default_tone_check(), true);
        // 序列化 camelCase + round-trip 保留显式关闭
        let v = serde_json::to_value(Settings { tone_check: false, ..Default::default() }).unwrap();
        assert_eq!(v["toneCheck"], false);
        assert!(!serde_json::from_value::<Settings>(v).unwrap().tone_check);
    }

    #[test]
    fn ai_nudge_dismissed_defaults_false_and_serializes_camel() {
        // 旧 settings.json 无该字段 → false（主界面显示「AI 智能层未开启」横幅）
        let legacy = r#"{ "aiBackend": "openai", "apiKey": "k", "baseUrl": "", "modelName": "", "obsidianVaultPath": "", "scenario": "free" }"#;
        let s: Settings = serde_json::from_str(legacy).unwrap();
        assert!(!s.ai_nudge_dismissed);
        assert!(!Settings::default().ai_nudge_dismissed);
        // 序列化 camelCase（与前端 store 键一致）+ round-trip 保留「暂不提醒」
        let v =
            serde_json::to_value(Settings { ai_nudge_dismissed: true, ..Default::default() })
                .unwrap();
        assert_eq!(v["aiNudgeDismissed"], true);
        assert!(serde_json::from_value::<Settings>(v).unwrap().ai_nudge_dismissed);
        // 非布尔值（异常数据）→ serde default false 兜底由前端 normalize 负责，Rust 侧仅约定布尔
    }
}
