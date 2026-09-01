param(
    # 附带旧 Zipformer 模型（仅 A/B 对比用；日常使用不需要）
    [switch]$IncludeZipformer,
    # 附带 SenseVoice 高精终稿引擎（可选；不装则终稿用流式引擎）
    [switch]$IncludeSenseVoice
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$root = Split-Path -Parent $PSScriptRoot
$models = Join-Path $root "models"
New-Item -ItemType Directory -Force -Path $models | Out-Null

# GitHub 直连不可达时自动走镜像（ghfast.top 为 GitHub release 反代；按需增删）
$mirrors = @(
    "https://github.com/k2-fsa/sherpa-onnx/releases/download",
    "https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download"
)

function Get-ReleaseAsset {
    param($RelativePath, $OutFile)
    if (Test-Path $OutFile) { return }
    foreach ($base in $mirrors) {
        $url = "$base/$RelativePath"
        try {
            Write-Host "下载 $url ..."
            Invoke-WebRequest -Uri $url -OutFile $OutFile -TimeoutSec 600
            return
        } catch {
            Write-Warning "下载失败: $url ($($_.Exception.Message))，尝试下一个源 ..."
        }
    }
    throw "所有下载源均失败: $RelativePath"
}

# 1) Silero VAD（断句）
$vad = Join-Path $models "silero_vad.onnx"
if (-not (Test-Path $vad)) {
    Get-ReleaseAsset "asr-models/silero_vad.onnx" $vad
}

# 2) 流式 Paraformer 中英双语 int8（主识别模型）
$para = Join-Path $models "sherpa-onnx-streaming-paraformer-bilingual-zh-en"
if (-not (Test-Path (Join-Path $para "tokens.txt"))) {
    $pkg = Join-Path $models "paraformer.tar.bz2"
    Get-ReleaseAsset "asr-models/sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2" $pkg
    Write-Host "解压 ..."
    tar -xjf $pkg -C $models
    Remove-Item $pkg
    # 应用只用 int8；发布包里同时附带的 fp32 权重删掉省磁盘（约 1GB+）
    Get-ChildItem $para -Filter "*.onnx" | Where-Object { $_.Name -notmatch "int8" } | Remove-Item -Force
}
Write-Host "识别模型就绪: $para"

# 3) SenseVoice 离线高精引擎（默认跳过；-IncludeSenseVoice 下载）
if ($IncludeSenseVoice) {
    $senseDirName = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"
    $sense = Join-Path $models $senseDirName
    if (-not (Test-Path (Join-Path $sense "model.int8.onnx"))) {
        $pkg = Join-Path $models "sense-voice.tar.bz2"
        Get-ReleaseAsset "asr-models/$senseDirName.tar.bz2" $pkg
        Write-Host "解压 ..."
        tar -xjf $pkg -C $models
        Remove-Item $pkg
        # 只留 int8（fp32 权重约 900MB，删掉省磁盘）
        Get-ChildItem $sense -Filter "*.onnx" | Where-Object { $_.Name -notmatch "int8" } | Remove-Item -Force
    }
    Write-Host "高精引擎就绪: $sense"
}

# 4) 旧流式 Zipformer（默认跳过；A/B 对比时用 -IncludeZipformer 下载）
if ($IncludeZipformer) {
    $zip = Join-Path $models "zipformer"
    if (-not (Test-Path (Join-Path $zip "tokens.txt"))) {
        New-Item -ItemType Directory -Force -Path $zip | Out-Null
        $pkg = Join-Path $models "zipformer-bilingual.tar.bz2"
        Get-ReleaseAsset "asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2" $pkg
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
    Write-Host "旧模型就绪: $zip"
}

Write-Host "模型就绪：$models"
