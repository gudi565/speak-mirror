# 表达训练系统
实时反馈的中文口头表达训练工具：说话时显示字幕、标红口头禅、给出词汇/结构提醒。

## 环境要求

- Node 18+、Rust 1.75+（Windows 需 MSVC Build Tools）

## 首次运行

1. `npm install`
2. `powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1`（下载 VAD + 流式 ASR 模型，约 340MB）
3. `npm run tauri dev`

## 测试

- Rust 规则引擎：`cd src-tauri && cargo test`
- 前端：`npm test`
