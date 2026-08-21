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
