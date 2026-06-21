<#
.SYNOPSIS
  Optional: download the offline voice-typing engine (whisper.cpp) + a model into
  C:\ProgramData\WarmupVk\speech so the companion's on-screen Mic key appears.

.DESCRIPTION
  The companion shows the Mic key only when BOTH whisper-server.exe and a GGML
  model are present in the speech dir (see src\win\speech_input.rs::available).
  Without them the key stays hidden. This is a pure opt-in: nothing here is
  required for the companion to run. Re-run any time to add or change the model.

  Manual fallback (if a download fails): drop any ggml-*.bin and a whisper.cpp
  whisper-server.exe (+ its DLLs) into the speech dir yourself — same result.

  Models (multilingual, incl. German): tiny ~75MB, base ~142MB, small ~466MB,
  medium ~1.5GB. Bigger = more accurate, slower per utterance.
#>
param(
    [ValidateSet("tiny", "base", "small", "medium")]
    [string]$Model = "medium",
    [string]$Dest = "C:\ProgramData\WarmupVk\speech",
    # whisper.cpp release tag whose whisper-bin-x64.zip ships whisper-server.exe
    # (verified: the CPU x64 zip contains whisper-server.exe + ggml*/whisper DLLs).
    [string]$WhisperRelease = "v1.9.1"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # 5.1 IWR is painfully slow with the progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force $Dest | Out-Null

# The mic helper runs as the (non-elevated) logged-in user, but the data dir is
# locked to SYSTEM+Administrators. Grant Users read+execute on just this speech
# dir (by SID, locale-independent) or the helper can't read the model/runner.
icacls $Dest /grant:r "*S-1-5-32-545:(OI)(CI)RX" | Out-Null

# Runtime signal dir (status / stop): the helper writes here, so Users need
# write — kept separate from the read-only engine dir so the exe stays protected.
$Rt = Join-Path $Dest "rt"
New-Item -ItemType Directory -Force $Rt | Out-Null
icacls $Rt /grant:r "*S-1-5-32-545:(OI)(CI)M" | Out-Null

# Streamed download with periodic % progress to stdout. Invoke-WebRequest hides
# progress under nsExec (and SilentlyContinue), so a big model looked frozen;
# nsExec::ExecToLog shows these Write-Host lines live. Downloads to a .part file
# and renames on success, so a failed/partial download isn't mistaken for "present".
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

# 1) Model — HuggingFace resolve URL is stable across whisper.cpp versions.
$modelFile = Join-Path $Dest "ggml-$Model.bin"
if (Test-Path $modelFile) {
    Write-Host "Model already present: $modelFile"
} else {
    $modelUrl = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$Model.bin?download=true"
    Write-Host "Downloading whisper model '$Model'..."
    Save-WithProgress -Url $modelUrl -OutFile $modelFile -Label "model $Model"
}

# Record the chosen model so the helper loads THIS one even when several *.bin
# files are present (companion reads speech\model.txt; see speech_input::model_path).
Set-Content -Path (Join-Path $Dest "model.txt") -Value "ggml-$Model.bin" -NoNewline -Encoding ascii

# 2) Runner — whisper-server.exe (+ DLLs) from the whisper.cpp Windows release.
$serverExe = Join-Path $Dest "whisper-server.exe"
if (Test-Path $serverExe) {
    Write-Host "Runner already present: $serverExe"
} else {
    $zipUrl = "https://github.com/ggerganov/whisper.cpp/releases/download/$WhisperRelease/whisper-bin-x64.zip"
    $tmpZip = Join-Path $env:TEMP "warmup-whisper-bin.zip"
    $tmpDir = Join-Path $env:TEMP "warmup-whisper-bin"
    Write-Host "Downloading whisper.cpp runner $WhisperRelease..."
    Save-WithProgress -Url $zipUrl -OutFile $tmpZip -Label "runner"
    if (Test-Path $tmpDir) { Remove-Item $tmpDir -Recurse -Force }
    Expand-Archive -Path $tmpZip -DestinationPath $tmpDir -Force
    # Tolerant: the server exe has been named server.exe or whisper-server.exe
    # across releases. Take whichever exists; copy it plus every DLL it needs.
    $srv = Get-ChildItem $tmpDir -Recurse -Include "whisper-server.exe", "server.exe" |
        Select-Object -First 1
    if (-not $srv) {
        throw "no whisper server exe in $zipUrl. Place whisper-server.exe + a ggml-*.bin in $Dest manually."
    }
    Copy-Item $srv.FullName $serverExe -Force
    Get-ChildItem $tmpDir -Recurse -Filter *.dll |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $Dest $_.Name) -Force }
    Remove-Item $tmpZip -Force
    Remove-Item $tmpDir -Recurse -Force
}

Write-Host "Offline voice typing ready in $Dest. The Mic key appears next time the keyboard opens."
