# SpeakMirror（表达镜）

**离线优先的开源中文表达训练工具：说话时实时看字幕、标红口头禅，还听你的声音、记得你的进步——不用联网、不收钱。**

SpeakMirror 是一个跑在你自己电脑上的表达陪练：点「开始练习」随便说一段话，左边看实时统计与声音仪表，中间滚动字幕并标红口头禅，右边给出词汇与结构提醒；说完一键生成逐句改写的 AI 报告。语音识别与规则分析全部在本地完成。

## 核心功能

- **三栏实时反馈**：本地流式识别（中英双语）实时出字幕；口头禅在字幕中标红；右栏即时提醒词汇精确度、重复、结论缺失、立场模糊、时间模糊等问题
- **词库规则引擎**：内置 MIT 授权的自建词库（439 情绪词、126 组笼统→精准替换、45 个分级填充词），9 条实时规则每条可单独关闭，同类提醒带冷却窗口——宁可漏报，不可刷屏
- **多后端 AI 报告**：练习结束生成结构化报告（评分定位 / 亮点 / 逐句改写 / 可替换词汇 / 行为模式 / 对比上次 / 下次重点），支持 DeepSeek / OpenAI / Groq / Ollama 及任意 OpenAI 兼容端点；不配 Key 也有本地降级报告（纯统计 + 规则汇总）；自由练习与面试回答（STAR）两套场景模板；0.2.3 起可选「快速报告」（四节 ≤400 字，约 1/3 生成时间，默认）或完整八节报告，生成过程骨架逐节点亮
- **声音层分析**：不只看「说了什么」，还看「怎么说的」——语速、失控停顿、音量动态范围、能量稳定性，全部为会话内相对值，不受麦克风差异影响
- **声调提示（v0.2.4 起，v0.2.6 支持三声变调与音域归一）**：练习结束后对本机录音做普通话声调偏差分析（纯本地 DSP，无需联网），逐句定位「应为几声」，每条提示可一键回放该句试听；多重置信门控，宁可漏报不误报
- **成长档案**：每次练习自动落盘，历史页查看趋势（口头禅频率 / 语速 / 停顿）与目标达成情况，报告自动「对比上次」

## 英文练习支持 / English Practice

直接说英文即可：识别、实时规则（英文口头禅/词汇精确度/立场模糊，按词边界匹配）与 AI 报告（英文八节/快速报告模板）自动切换，无需设置；界面语言保持中文，普通话声调分析对英文练习自动隐藏。
Just speak English: recognition, real-time rules (English fillers / vocabulary precision / hedging, matched on word boundaries) and AI reports (English full/quick templates) switch automatically — no configuration needed; the UI stays in Chinese and Mandarin tone analysis hides itself for English sessions.

## 安装（Windows / macOS）

### Windows

1. 从 [Releases](../../releases) 下载 `SpeakMirror_0.2.7_x64-setup.exe`
2. 双击安装（安装语言可选简体中文 / English，默认安装到当前用户目录，无需管理员权限）
3. 启动后进入首启引导，按提示完成即可开始练习

### macOS（Apple Silicon 与 Intel 均可）

1. 从 [Releases](../../releases) 下载 `SpeakMirror_..._universal.dmg`
2. 打开 dmg，将 SpeakMirror 拖入「应用程序」文件夹
3. **首次打开**：因未购买开发者签名证书，需在「应用程序」中**右键 → 打开**（或在「系统设置 → 隐私与安全性」点「仍要打开」）
4. 首次练习时 macOS 会弹麦克风权限对话框，点「允许」即可（音频不离开本机）

### 从源码构建（任意平台）

要求：Node 18+、Rust 1.75+（MSVC 工具链）、Windows 10/11。

```powershell
git clone https://github.com/gudi565/speak-mirror.git speak-mirror
cd speak-mirror
npm install
npm run tauri build
```

构建产物：`src-tauri\target\release\bundle\nsis\SpeakMirror_0.2.7_x64-setup.exe`（exe 本体在 `src-tauri\target\release\`）。

开发调试：`npm run tauri dev`。测试：`cd src-tauri && cargo test`（Rust 规则引擎）与 `npm test`（前端）。

> 首次 `cargo build` 时 `sherpa-onnx` crate 会自动从 GitHub Releases 下载 Windows 预编译静态库（约 120MB）。直连受限时可手动下载
> `sherpa-onnx-v1.13.6-win-x64-static-MT-Release-lib.tar.bz2` 放入任意目录，设置环境变量 `SHERPA_ONNX_ARCHIVE_DIR` 指向该目录。

## 首次运行引导

首次启动会进入三步向导：

1. **下载识别引擎**（自动，约几分钟）：推荐全量约 460MB，只需下载一次——实时识别引擎约 230MB（滚动字幕）+ 高精度终稿引擎 SenseVoice 约 230MB（每句定稿，精度与标点明显更好，默认必装）。网络受限时可显式选择「仅下载必需组件」（约 230MB，不推荐：定稿精度明显下降且不含标点），之后可在设置中随时补装。下载源自动按「ghfast.top 反代 → HuggingFace / hf-mirror」顺序尝试；中断后重试会断点续传。也可手动下载（见下）
2. **配置 AI 后端**（可跳过）：不配置也能完整使用实时反馈、声音仪表与本地降级报告；配置 DeepSeek / OpenAI / Groq / Ollama 后可解锁 AI 完整报告与练习中周期快评
3. **试录**：进入主界面，点「开始练习」说 30 秒试试

## 模型下载说明

实时识别必需两组模型（约 230MB 磁盘占用），另有高精度终稿引擎 SenseVoice（0.2.3 起默认随首启引导一起下载，推荐全量约 460MB）：

| 模型 | 作用 | 大小 | 许可 |
|---|---|---|---|
| Silero VAD | 语音活动检测（断句） | ~0.6MB | MIT |
| sherpa-onnx 流式 Paraformer 中英双语 int8 | 实时字幕（流式识别） | ~230MB | Apache-2.0 |
| sherpa-onnx SenseVoice 中英日韩粤 int8 | 高精度终稿（双引擎定稿，自带标点与 ITN），默认安装 | ~230MB | Apache-2.0 |

必需模型（Silero VAD + 流式 Paraformer）是应用可运行的最小集合；SenseVoice 高精引擎用于句子定稿时用离线引擎重新识别整段，获得更高精度与标点。跳过 SenseVoice 需在首启引导中显式确认——句子定稿精度会明显下降（回退流式引擎终稿，不含标点），且可随时在「设置 → 识别组件」中补装。

应用内下载会自动删除归档中的 fp32 权重只保留 int8（Paraformer 原始压缩包约 1GB、SenseVoice 约 1GB，解压清理后各 230MB 左右）。

手动下载（源码运行方式）：项目根目录执行（高精引擎加 `-IncludeSenseVoice`）

```powershell
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
```

模型放置位置：源码运行为项目根 `models\`；安装版在应用数据目录（`%APPDATA%\com.speakmirror.desktop\models`，由首启引导自动管理）。

## 竞品对照

| 能力 | SpeakMirror | 同类开源项目 | SayNow | Yoodli |
|---|---|---|---|---|
| 实时识别 | 本地流式 | 本地流式 | 云端 | 云端 |
| 实时词库反馈 | ✅ | ✅ | 部分 | ✅ |
| AI 实时介入 | ✅ 批量 | ✅ 50 字 | ✅ | ✅ |
| 分析报告 | ✅ 多后端+场景 | ✅ 多后端 | ✅ | ✅ |
| **声音层分析** | ✅ 本地 | ❌ | ✗ | ✅ 云端 |
| **成长档案** | ✅ | ❌ | ✅ 路径 | ✅ |
| 离线/隐私 | 实时层全离线 | 同 | ✗ | ✗ |
| 体积/内存 | Tauri，安装 <500MB | Electron，首载 1.5GB | 浏览器 | 浏览器 |
| 价格 | 免费开源 | 免费开源 | 订阅 | 订阅 |

## 隐私与免责

- **语音全程本地处理**：麦克风音频、识别、断句、规则分析均在本机完成，不上传任何语音数据
- **会话录音仅存本机**：用于结束后逐句回放的会话录音只保存在本机应用数据目录，不上传、不联网；可在「设置」中关闭录音，删除历史记录时对应录音一并删除
- **API Key 仅存本地**：AI 报告为可选功能，Key 保存在本机应用数据目录，仅在你主动生成报告时用于直连所选 AI 服务商
- AI 报告内容仅供练习参考，不构成任何专业建议

## 致谢与开源许可

本项目的识别能力站在以下开源工作之上：

- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)（Apache-2.0）及其作者的预训练模型发布：流式 Paraformer 中英双语模型（Apache-2.0）、Silero VAD（MIT，[snakers4/silero-vad](https://github.com/snakers4/silero-vad)）
- [Tauri](https://tauri.app/)（MIT/Apache-2.0）、React（MIT）等依赖，完整清单见各锁文件
- 词库：种子数据来自 [expression-trainer](https://github.com/fxy2311-youyou/expression-trainer)（MIT）的 `emotion-lexicon.json`（约 200 条），其余条目为 SpeakMirror 自建扩充（来源与授权记录见 `docs/lexicon/README.md`），全部以 MIT 随本项目发布；未使用任何限制商用的词表数据

本项目自身以 [MIT](LICENSE) 发布。

---

SpeakMirror v0.2.7 · 免责：本地处理语音，API Key 仅存本地。
