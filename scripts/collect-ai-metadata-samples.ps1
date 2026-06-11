param(
    [string]$OutputDir = "$env:USERPROFILE\.codex\mimageviewer-ai-metadata-samples"
)

$ErrorActionPreference = "Stop"

$samples = @(
    @{
        Name = "novelai-sample-cat.png"
        Url = "https://raw.githubusercontent.com/tuki0918/novelai-png-metadata-cli/main/.docs/sample-cat.png"
    },
    @{
        Name = "swarmui-image-metadata-format.md"
        Url = "https://raw.githubusercontent.com/mcmonkeyprojects/SwarmUI/master/docs/Image%20Metadata%20Format.md"
    }
)

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

foreach ($sample in $samples) {
    $dest = Join-Path $OutputDir $sample.Name
    Write-Host "Downloading $($sample.Name)"
    Invoke-WebRequest -Uri $sample.Url -OutFile $dest -UseBasicParsing
}

Write-Host ""
Write-Host "Samples saved to: $OutputDir"
Write-Host "To enable optional smoke tests in this shell:"
Write-Host ('$env:MIV_AI_METADATA_SAMPLE_DIR = "' + $OutputDir + '"')
Write-Host "cargo test optional_external_novelai_sample_smoke -- --nocapture"
