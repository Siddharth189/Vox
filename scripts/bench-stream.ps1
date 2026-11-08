# bench-stream.ps1 - streaming accuracy on a long dictation (Windows CI).
#
#   scripts\bench-stream.ps1 -Vox target\release\vox.exe -Model <ggml model>
#
# Synthesizes a ~40 s, multi-sentence paragraph with the built-in Windows
# speech synthesizer (so no audio is checked in), transcribes it in one pass
# and streamed (played in real time), and fails if streaming repeats a
# sentence or its words drift from the one-pass transcript. Real Whisper
# found both on this kind of clip before: Whisper echoing the previous
# segment's last sentence, and cuts at comma pauses turning commas into
# full stops. The wait after stop is reported, not gated.
param(
    [Parameter(Mandatory = $true)][string]$Vox,
    [Parameter(Mandatory = $true)][string]$Model,
    [double]$MinSimilarity = 0.93
)
$ErrorActionPreference = 'Stop'

$text = 'Here is the plan for next week. First, we finish the Windows installer and get it signed, ' +
    'so people stop seeing the warning. Second, we move the speech model download into the background, ' +
    'with progress shown in the tray menu. Third, I want to look at memory use on smaller laptops, ' +
    'because the small model was swapping on an eight gigabyte machine. After that, we can start on ' +
    'command mode, where you select some text and say how to rewrite it. I think that is the feature ' +
    'people will actually notice. Let me know what you think, and whether anything should move up the list.'

$wav = Join-Path ([IO.Path]::GetTempPath()) 'vox-bench-long.wav'
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(48000,
    [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
    [System.Speech.AudioFormat.AudioChannel]::Mono)
$synth.SetOutputToWaveFile($wav, $format)
$synth.Speak($text)
$synth.Dispose()

function Get-Raw([string[]]$Lines) {
    $line = $Lines | Where-Object { $_ -like 'raw: *' } | Select-Object -First 1
    if (-not $line) { throw "no transcript in vox output:`n$($Lines -join "`n")" }
    $line.Substring(5)
}

function Get-Words([string]$s) {
    @($s.ToLowerInvariant() -split '[^\p{L}\p{N}]+' | Where-Object { $_ })
}

# Word-level similarity: 2 * LCS / (len a + len b).
function Get-Similarity([string[]]$a, [string[]]$b) {
    if ($a.Count + $b.Count -eq 0) { return 1.0 }
    $prev = New-Object int[] ($b.Count + 1)
    foreach ($x in $a) {
        $cur = New-Object int[] ($b.Count + 1)
        for ($j = 1; $j -le $b.Count; $j++) {
            if ($x -eq $b[$j - 1]) { $cur[$j] = $prev[$j - 1] + 1 }
            else { $cur[$j] = [Math]::Max($prev[$j], $cur[$j - 1]) }
        }
        $prev = $cur
    }
    2.0 * $prev[$b.Count] / ($a.Count + $b.Count)
}

# Sentences (normalized) that appear more often in $s than in $reference.
function Get-Repeats([string]$s, [string]$reference) {
    $split = { param($t) $t -split '(?<=[.!?])\s+' | ForEach-Object { (Get-Words $_) -join ' ' } | Where-Object { $_.Split(' ').Count -ge 4 } }
    $ref = @(& $split $reference)
    & $split $s | Group-Object | Where-Object {
        $name = $_.Name
        $_.Count -ge 2 -and $_.Count -gt @($ref | Where-Object { $_ -eq $name }).Count
    } | ForEach-Object { $_.Name }
}

# whisper.cpp logs to stderr; Windows PowerShell would turn that into
# errors under 'Stop'.
$ErrorActionPreference = 'Continue'
$one = & $Vox transcribe $wav --model $Model 2>$null
$stream = & $Vox transcribe $wav --model $Model --stream 2>$null
$ErrorActionPreference = 'Stop'
$oneRaw = Get-Raw $one
$streamRaw = Get-Raw $stream
$oneStt = $one | Where-Object { $_ -like 'stt: *' } | Select-Object -First 1
$streamStt = $stream | Where-Object { $_ -like 'stt: *' } | Select-Object -First 1
Write-Output "one pass: $oneStt"
Write-Output "  $oneRaw"
Write-Output "streamed: $streamStt"
Write-Output "  $streamRaw"

$similarity = Get-Similarity (Get-Words $oneRaw) (Get-Words $streamRaw)
$repeats = @(Get-Repeats $streamRaw $oneRaw)
Write-Output ("::notice title=Streaming STT (40 s paragraph)::{0} (one pass: {1}); word similarity {2:P1}" -f $streamStt, $oneStt, $similarity)

$failed = $false
if ($repeats.Count -gt 0) {
    Write-Output "::error title=Streaming repeated a sentence::$($repeats -join ' | ')"
    $failed = $true
}
$oneSentences = ([regex]::Matches($oneRaw, '[.!?](\s|$)')).Count
$streamSentences = ([regex]::Matches($streamRaw, '[.!?](\s|$)')).Count
if ($streamSentences -gt $oneSentences + 1) {
    Write-Output "::error title=Streaming split sentences::$streamSentences sentences vs $oneSentences in one pass (cuts at comma pauses?)"
    $failed = $true
}
if ($similarity -lt $MinSimilarity) {
    Write-Output ("::error title=Streamed transcript drifted::word similarity {0:P1} < {1:P0} vs one pass" -f $similarity, $MinSimilarity)
    $failed = $true
}
if ($failed) { exit 1 }
Write-Output 'streaming accuracy OK'
