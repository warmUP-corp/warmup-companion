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
    [string]$OrtVersion = "1.27.0",
    [string]$OrtDmlWheel = "https://files.pythonhosted.org/packages/88/ea/33814eb0ec96775eda4c1d30b0d86e91d7d2cd0d84c66d3915aef0e06fa3/onnxruntime_directml-1.24.4-cp312-cp312-win_amd64.whl",
    [ValidateSet("parakeet","redux")][string]$Variant = "parakeet"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # 5.1 IWR is painfully slow with the progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Classic = Join-Path $Dest "parakeet"
if ($Variant -eq "redux") {
    $Pk = Join-Path $Dest "parakeet-redux"
    $ModelRepo = "eschmidbauer/parakeet-redux-onnx"
    $Rev = "1c285ba78d6f1880803f65355133a5ad3559a407"
    $files = @("encoder-model.onnx", "decoder_joint-model.onnx", "vocab.txt")
    $EngineId = "parakeet-redux"
} else {
    $Pk = $Classic
    $Rev = "main"
    $files = @("encoder-model.int8.onnx", "decoder_joint-model.int8.onnx", "vocab.txt")
    $EngineId = "parakeet"
}
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
                    if ($pct -ge $lastPct + 1) {
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
foreach ($f in $files) {
    $out = Join-Path $Pk $f
    if (Test-Path $out) {
        Write-Host "Already present: $f"
    } else {
        $url = "https://huggingface.co/$ModelRepo/resolve/$Rev/$f`?download=true"
        Write-Host "Downloading $f..."
        Save-WithProgress -Url $url -OutFile $out -Label $f
    }
}

# 2) ONNX Runtime DLL — load-dynamic backend the helper points ORT_DYLIB_PATH at.
# From the NuGet .nupkg (a zip): runtimes/win-x64/native/onnxruntime.dll. The
# package also carries win-x86/arm64 copies, so filter to win-x64.
function Save-ZipFiles {
    param([string]$Url, [string]$Label, [hashtable]$Entries)
    $tmpZip = Join-Path $env:TEMP "warmup-$Label.zip"
    Write-Host "Downloading $Label..."
    Save-WithProgress -Url $Url -OutFile $tmpZip -Label $Label
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($tmpZip)
    try {
        foreach ($Entry in $Entries.Keys) {
            $OutFile = $Entries[$Entry]
            $item = $zip.Entries | Where-Object { $_.FullName -eq $Entry } | Select-Object -First 1
            if (-not $item) { throw "$Entry not found in $Url" }
            $tmpOut = "$OutFile.new"
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($item, $tmpOut, $true)
            if (Test-Path $OutFile) {
                $old = "$OutFile.old"
                if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }
                Move-Item -LiteralPath $OutFile -Destination $old -Force
                Remove-Item $old -Force -ErrorAction SilentlyContinue
            }
            Move-Item -LiteralPath $tmpOut -Destination $OutFile -Force
        }
    } finally { $zip.Dispose() }
    Remove-Item $tmpZip -Force
}

$dll = Join-Path $Pk "onnxruntime.dll"
$dmlDll = Join-Path $Pk "DirectML.dll"
$classicDll = Join-Path $Classic "onnxruntime.dll"
$classicDml = Join-Path $Classic "DirectML.dll"
if ((Test-Path $dll) -and (Test-Path $dmlDll)) {
    Write-Host "Runtime already present: onnxruntime.dll + DirectML.dll"
} elseif ($Pk -ne $Classic -and (Test-Path $classicDll) -and (Test-Path $classicDml)) {
    Write-Host "Copying the DirectML runtime from $Classic"
    Copy-Item $classicDll $dll -Force
    Copy-Item $classicDml $dmlDll -Force
} else {
    Save-ZipFiles -Url $OrtDmlWheel -Label "onnxruntime-directml" -Entries @{
        "onnxruntime/capi/onnxruntime.dll" = $dll
        "onnxruntime/capi/DirectML.dll" = $dmlDll
    }
}

if ($Variant -eq "redux") {
    Add-Type -TypeDefinition @"
public static class WarmupOnnxPatch {
    public static int Patch(byte[] d) {
        byte[] p = { 0x0a, 0x09, 0x61, 0x6c, 0x6c, 0x6f, 0x77, 0x7a, 0x65, 0x72, 0x6f, 0x18, 0x01 };
        int n = 0;
        for (int i = 0; i <= d.Length - p.Length; i++) {
            int j = 0;
            while (j < p.Length && d[i + j] == p[j]) j++;
            if (j == p.Length) { d[i + p.Length - 1] = 0; n++; i += p.Length - 1; }
        }
        return n;
    }
}
"@
    $enc = Join-Path $Pk "encoder-model.onnx"
    $bytes = [System.IO.File]::ReadAllBytes($enc)
    $n = [WarmupOnnxPatch]::Patch($bytes)
    if ($n -gt 0) {
        [System.IO.File]::WriteAllBytes("$enc.new", $bytes)
        Move-Item -LiteralPath "$enc.new" -Destination $enc -Force
    }
    Write-Host "Patched $n reshape nodes for DirectML"
}

# 3) Make Parakeet the active engine. The tray "Voice engine" menu can switch back.
Set-Content -Path (Join-Path $Dest "engine.txt") -Value $EngineId -NoNewline -Encoding ascii

Write-Host "Parakeet voice typing ready in $Pk. The Mic key appears next time the keyboard opens."
