param(
    [string]$OutputDir = "H:\home\mimageviewer_old\testimage\metadata"
)

$ErrorActionPreference = "Stop"

# AI metadata real-world samples for optional smoke tests
# (src/png_metadata.rs: optional_external_*_smoke).
# Sources:
# - tuki0918/novelai-png-metadata-cli (NovelAI v3 sample)
# - d3x-at/sd-parsers, MIT (tests/resources: A1111 png/jpg/stealth, Fooocus,
#   InvokeAI dream/imeta/sdmeta, NovelAI, zTXt-after-IDAT edge case)
# - mcmonkeyprojects/SwarmUI (metadata format doc, reference only)
$sdparsersBase = "https://raw.githubusercontent.com/d3x-at/sd-parsers/master/tests/resources"
$samples = @(
    @{
        Name = "novelai-sample-cat.png"
        Url = "https://raw.githubusercontent.com/tuki0918/novelai-png-metadata-cli/main/.docs/sample-cat.png"
    },
    @{
        Name = "swarmui-image-metadata-format.md"
        Url = "https://raw.githubusercontent.com/mcmonkeyprojects/SwarmUI/master/docs/Image%20Metadata%20Format.md"
    },
    @{
        Name = "sdparsers-automatic1111_cropped.png"
        Url = "$sdparsersBase/parsers/AUTOMATIC1111/automatic1111_cropped.png"
    },
    @{
        Name = "sdparsers-automatic1111_cropped.jpg"
        Url = "$sdparsersBase/parsers/AUTOMATIC1111/automatic1111_cropped.jpg"
    },
    @{
        Name = "sdparsers-automatic1111_stealth.png"
        Url = "$sdparsersBase/parsers/AUTOMATIC1111/automatic1111_stealth.png"
    },
    @{
        Name = "sdparsers-fooocus1_cropped.png"
        Url = "$sdparsersBase/parsers/Fooocus/fooocus1_cropped.png"
    },
    @{
        Name = "sdparsers-invokeai_dream1.png"
        Url = "$sdparsersBase/parsers/InvokeAI/invokeai_dream1.png"
    },
    @{
        Name = "sdparsers-invokeai_imeta1.png"
        Url = "$sdparsersBase/parsers/InvokeAI/invokeai_imeta1.png"
    },
    @{
        Name = "sdparsers-invokeai_sdmeta1.png"
        Url = "$sdparsersBase/parsers/InvokeAI/invokeai_sdmeta1.png"
    },
    @{
        Name = "sdparsers-novelai1_cropped.png"
        Url = "$sdparsersBase/parsers/NovelAI/novelai1_cropped.png"
    },
    @{
        Name = "sdparsers-text_after_idat.png"
        Url = "$sdparsersBase/bad_images/text_after_idat.png"
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
Write-Host "cargo test --bin mimageviewer-core optional_external_ -- --nocapture"
