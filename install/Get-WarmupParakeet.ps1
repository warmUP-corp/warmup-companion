<#
.SYNOPSIS
  Optional: download the NVIDIA Parakeet voice-typing engine (ONNX) into
  C:\ProgramData\WarmupVk\speech\parakeet so the companion can use it instead of
  whisper.cpp. Sibling of Get-WarmupSpeech.ps1.

.DESCRIPTION
  Parakeet is a SECOND, separate dictation engine. Unlike whisper there is no
  server: the companion's speech helper loads the model in-process via parakeet-rs
  (ort/ONNX Runtime). The companion picks the engine from speech\engine.txt (set
  here to "parakeet"); the tray "Voice engine" menu can switch back to whisper.

  Downloads the int8 TDT model (~670 MB, multilingual incl. German) plus a
  matching onnxruntime.dll (loaded dynamically at runtime). The Mic key + engine
  menu appear only once all files are present (src\win\speech_input.rs::engine::
  parakeet::available). Pure opt-in; re-run any time. Manual fallback: drop
  encoder-model.int8.onnx, decoder_joint-model.int8.onnx, vocab.txt and
  onnxruntime.dll into the parakeet dir yourself.

  Files come from HuggingFace istupakov/parakeet-tdt-0.6b-v3-onnx and the official
  microsoft/onnxruntime release.
#>
param(
    [string]$Dest = "C:\ProgramData\WarmupVk\speech",
    # HuggingFace repo holding the ONNX TDT model (encoder/decoder_joint + vocab).
    [string]$ModelRepo = "istupakov/parakeet-tdt-0.6b-v3-onnx",
    # ONNX Runtime version (CPU). Pulled from the Microsoft.ML.OnnxRuntime NuGet
    # package, which always ships runtimes/win-x64/native/onnxruntime.dll (the
    # GitHub release zips are GPU/CUDA-only for recent versions). ort 2.0.0-rc.12
    # targets ORT 1.24 (api-24); the C API is backward-compatible within 1.x, so a
    # newer stable DLL also satisfies it. Bump if ort's required api version moves.
    [string]$OrtVersion = "1.27.0"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # 5.1 IWR is painfully slow with the progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Pk = Join-Path $Dest "parakeet"
New-Item -ItemType Directory -Force $Pk | Out-Null

# Same ACL story as Get-WarmupSpeech.ps1: the mic helper runs as the non-elevated
# logged-in user but the data dir is locked to SYSTEM+Administrators. Grant Users
# read+execute on the speech dir (by SID, locale-independent) so the helper can
# read the model + DLL. (OI)(CI) inherits into the parakeet subdir.
icacls $Dest /grant:r "*S-1-5-32-545:(OI)(CI)RX" | Out-Null
# Runtime signal dir (status / stop / level): the helper writes here, so Users need
# write — kept separate from the read-only engine dir so the binaries stay protected.
$Rt = Join-Path $Dest "rt"
New-Item -ItemType Directory -Force $Rt | Out-Null
icacls $Rt /grant:r "*S-1-5-32-545:(OI)(CI)M" | Out-Null

# Streamed download with periodic % progress to stdout (nsExec::ExecToLog shows
# these live). Downloads to .part and renames on success, so a partial download
# isn't mistaken for "present". Same helper as Get-WarmupSpeech.ps1.
function Save-WithProgress {
    param([string]$Url, [string]$OutFile, [string]$Label)
    $tmp = "$OutFile.part"
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.AllowAutoRedirect = $true
    $req.UserAgent = "warmup-companion"
    $resp = $req.GetResponse()
    try {
        $total = [double]$resp.ContentLength
        $in = $resp.GetResponseStream()
        $out = [System.IO.File]::Create($tmp)
        try {
            $buf = New-Object byte[] (1MB)
            $sofar = 0.0; $lastPct = -1; $lastMb = -1
            while (($n = $in.Read($buf, 0, $buf.Length)) -gt 0) {
                $out.Write($buf, 0, $n); $sofar += $n
                if ($total -gt 0) {
                    $pct = [int](100 * $sofar / $total)
                    if ($pct -ge $lastPct + 5) {
                        Write-Host ("  {0}: {1}% ({2:n0}/{3:n0} MB)" -f $Label, $pct, ($sofar / 1MB), ($total / 1MB))
                        $lastPct = $pct
                    }
                } elseif ([int]($sofar / 1MB) -ge $lastMb + 25) {
                    $lastMb = [int]($sofar / 1MB)
                    Write-Host ("  {0}: {1} MB..." -f $Label, $lastMb)
                }
            }
        } finally { $out.Dispose() }
    } finally { $resp.Dispose() }
    Move-Item -LiteralPath $tmp -Destination $OutFile -Force
}

# 1) Model — int8 TDT files + vocab. HuggingFace resolve URLs are stable. int8 is
# self-contained (no external .onnx.data); parakeet-rs's TDT loader accepts the
# .int8.onnx names directly (see model_tdt::find_encoder / find_decoder_joint).
$files = @("encoder-model.int8.onnx", "decoder_joint-model.int8.onnx", "vocab.txt")
foreach ($f in $files) {
    $out = Join-Path $Pk $f
    if (Test-Path $out) {
        Write-Host "Already present: $f"
    } else {
        $url = "https://huggingface.co/$ModelRepo/resolve/main/$f`?download=true"
        Write-Host "Downloading $f..."
        Save-WithProgress -Url $url -OutFile $out -Label $f
    }
}

# 2) ONNX Runtime DLL — load-dynamic backend the helper points ORT_DYLIB_PATH at.
# From the NuGet .nupkg (a zip): runtimes/win-x64/native/onnxruntime.dll. The
# package also carries win-x86/arm64 copies, so filter to win-x64.
$dll = Join-Path $Pk "onnxruntime.dll"
if (Test-Path $dll) {
    Write-Host "Runtime already present: onnxruntime.dll"
} else {
    $nupkgUrl = "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/$OrtVersion"
    $tmpZip = Join-Path $env:TEMP "warmup-ort.zip"   # .zip so Expand-Archive accepts it
    $tmpDir = Join-Path $env:TEMP "warmup-ort"
    Write-Host "Downloading ONNX Runtime $OrtVersion (NuGet)..."
    Save-WithProgress -Url $nupkgUrl -OutFile $tmpZip -Label "onnxruntime"
    if (Test-Path $tmpDir) { Remove-Item $tmpDir -Recurse -Force }
    Expand-Archive -Path $tmpZip -DestinationPath $tmpDir -Force
    $found = Get-ChildItem $tmpDir -Recurse -Filter "onnxruntime.dll" |
        Where-Object { $_.FullName -match "win-x64" } | Select-Object -First 1
    if (-not $found) {
        throw "no win-x64 onnxruntime.dll in the NuGet package. Place onnxruntime.dll in $Pk manually."
    }
    Copy-Item $found.FullName $dll -Force
    Remove-Item $tmpZip -Force
    Remove-Item $tmpDir -Recurse -Force
}

# 3) Make Parakeet the active engine. The tray "Voice engine" menu can switch back.
Set-Content -Path (Join-Path $Dest "engine.txt") -Value "parakeet" -NoNewline -Encoding ascii

Write-Host "Parakeet voice typing ready in $Pk. The Mic key appears next time the keyboard opens."
