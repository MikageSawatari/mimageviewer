# Assemble a verified kit. Never builds or launches mImageViewer.
[CmdletBinding()]
param(
    [string]$OutputRoot='C:\home\mimageviewer-subpc\relcheck',
    [string]$ArtifactRepo=(Join-Path $PSScriptRoot '..\..'),
    [string]$FfmpegPath
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2
. (Join-Path $PSScriptRoot 'KitSupport.ps1')
$scriptsRepo=(Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$ArtifactRepo=[IO.Path]::GetFullPath((Resolve-Path -LiteralPath $ArtifactRepo).Path).TrimEnd('\','/')
. (Join-Path $scriptsRepo 'scripts\ui-smoke\release-checks\Support.ps1')

function Get-KitGitOutput {
    param([string]$Root,[string[]]$GitArguments)
    $result=@(& git -C $Root @GitArguments)
    if($LASTEXITCODE -ne 0){throw "git failed in $Root"}
    return $result
}
function Copy-KitTree {
    param([string]$Source,[string]$Destination,[switch]$Scripts,[switch]$Package)
    Assert-ReleaseCheckKitNoReparsePath $Source
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    foreach($item in (Get-ChildItem -LiteralPath $Source -Force -ErrorAction Stop)){
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw "Copy refuses reparse point: $($item.FullName)"}
        if($Scripts -and (($item.PSIsContainer -and $item.Name -eq '__pycache__') -or
            (-not $item.PSIsContainer -and $item.Name -like '*.pyc'))){continue}
        if($Package -and $item.PSIsContainer -and $item.Name -eq 'data'){continue}
        $dest=Join-Path $Destination $item.Name
        if($item.PSIsContainer){Copy-KitTree $item.FullName $dest -Scripts:$Scripts}
        else{Copy-Item -LiteralPath $item.FullName -Destination $dest -ErrorAction Stop}
    }
}

if(@(Get-KitGitOutput $scriptsRepo @('status','--porcelain','--untracked-files=all','--','scripts')).Count){throw 'Kit scripts must be committed and clean.'}
$scriptsHead=[string](Get-KitGitOutput $scriptsRepo @('rev-parse','HEAD'))
if(@(Get-KitGitOutput $ArtifactRepo @('status','--porcelain','--untracked-files=all','--','scripts/build-portable.ps1')).Count){throw 'Artifact scripts/build-portable.ps1 must be committed and clean.'}
Import-ReleaseCheckFunctions (Join-Path $ArtifactRepo 'scripts\build-portable.ps1') @('Get-MivOrdinalUniquePaths','Get-MivSourceFingerprintRecords','Get-MivSourceFingerprint','Assert-SmokeBuildManifest','Get-NormalizedPath')
# Reuse the fingerprint function's exact source path list rather than a second list.
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $ArtifactRepo 'scripts\build-portable.ps1'),[ref]$tokens,[ref]$errors)
$function=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-MivSourceFingerprintRecords'},$true)
$assignments=@($function.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$sourcePaths'},$true))
if($assignments.Count -ne 1){throw 'Cannot identify fingerprint source path list.'}
$sourcePaths=@(& ([scriptblock]::Create($assignments[0].Right.Extent.Text)))
if(@(Get-KitGitOutput $ArtifactRepo (@('status','--porcelain','--untracked-files=all','--')+$sourcePaths)).Count){throw 'Artifact fingerprint source paths must be committed and clean.'}
$artifactHead=[string](Get-KitGitOutput $ArtifactRepo @('rev-parse','HEAD'))
$records=@(Get-MivSourceFingerprintRecords $ArtifactRepo)
$fingerprint=Get-MivSourceFingerprint $ArtifactRepo
$package=Join-Path $ArtifactRepo 'target\portable-smoke-package'
Assert-ReleaseCheckKitNoReparsePath $package
try{
    Assert-SmokeBuildManifest (Join-Path $package '.test-script-build.json') $fingerprint `
        (Join-Path $package 'mimageviewer.exe') (Join-Path $package 'mimageviewer-remote.exe') (Join-Path $package 'mimageviewer-epub-pdf.exe') `
        (Join-Path $package 'mimageviewer-mcp.exe') (Join-Path $package 'mimageviewer-musicvideo.exe')
}catch{throw ('Invalid diagnostic package; rebuild with prepare-portable-smoke.ps1 -TestScript. '+$_.Exception.Message)}
$artifactManifest=Get-Content -LiteralPath (Join-Path $package '.test-script-build.json') -Raw -Encoding UTF8 | ConvertFrom-Json
Assert-ReleaseCheckDiagnosticBuildRoot (Join-Path $package 'mimageviewer.exe') $ArtifactRepo
if(-not $FfmpegPath){$FfmpegPath=(Get-Command ffmpeg.exe -ErrorAction Stop).Source}
$FfmpegPath=(Resolve-Path -LiteralPath $FfmpegPath).Path
Assert-ReleaseCheckKitNoReparsePath $FfmpegPath
$ffmpegVersion=@(& $FfmpegPath -version)
if($LASTEXITCODE -ne 0 -or -not $ffmpegVersion.Count){throw 'ffmpeg -version failed.'}
$OutputRoot=[IO.Path]::GetFullPath($OutputRoot)
Assert-ReleaseCheckKitNoReparsePath $OutputRoot
foreach($source in @((Join-Path $scriptsRepo 'scripts'),$package)){
    $prefix=(Get-NormalizedPath $source)+'\'
    if(((Get-NormalizedPath $OutputRoot)+'\').StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)){throw 'OutputRoot must be outside copied source trees.'}
}
$kitId='relcheck-'+$artifactHead.Substring(0,9)+'-'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')
$kitRoot=Join-Path $OutputRoot $kitId
if(Test-Path -LiteralPath $kitRoot){throw "Kit already exists: $kitRoot"}
New-Item -ItemType Directory -Path $kitRoot -Force | Out-Null
Copy-KitTree (Join-Path $scriptsRepo 'scripts') (Join-Path $kitRoot 'scripts') -Scripts
Copy-KitTree $package (Join-Path $kitRoot 'target\portable-smoke-package') -Package
$tools=Join-Path $kitRoot 'tools\ffmpeg'
New-Item -ItemType Directory -Path $tools -Force | Out-Null
Copy-Item -LiteralPath $FfmpegPath -Destination (Join-Path $tools 'ffmpeg.exe')
foreach($dll in (Get-ChildItem -LiteralPath (Split-Path -Parent $FfmpegPath) -Filter '*.dll' -File -Force)){
    Assert-ReleaseCheckKitNoReparsePath $dll.FullName
    Copy-Item -LiteralPath $dll.FullName -Destination $tools
}
$utf8=New-Object Text.UTF8Encoding($false)
$recordPath=Join-Path $kitRoot 'source-fingerprint-records.txt'
[IO.File]::WriteAllText($recordPath,($records -join "`n"),$utf8)
if((Get-FileHash -LiteralPath $recordPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne $fingerprint){throw 'Source records changed while preparing kit.'}
$kit=[ordered]@{
    schema_version=1;kit_id=$kitId;created_utc=[DateTime]::UtcNow.ToString('o');scripts_source_head=$scriptsHead
    artifact=@{source_head=$artifactHead;source_fingerprint_sha256=$fingerprint;manifest=$artifactManifest;build_root=$ArtifactRepo}
    ffmpeg=@{version=[string]$ffmpegVersion[0];sha256=(Get-FileHash -LiteralPath (Join-Path $tools 'ffmpeg.exe') -Algorithm SHA256).Hash.ToLowerInvariant()}
    files=@(Get-ReleaseCheckKitFileRecords $kitRoot)
}
$json=Join-Path $kitRoot 'release-check-kit.json'
[IO.File]::WriteAllText($json,($kit | ConvertTo-Json -Depth 20),$utf8)
$digest=(Get-FileHash -LiteralPath $json -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $kitRoot 'release-check-kit.sha256'),$digest+"`n",$utf8)
$null=Assert-ReleaseCheckKit $kitRoot
Write-Host "Kit: $kitRoot"
Write-Host "Kit manifest SHA-256: $digest"
Write-Host "Run root on the sub-PC: $ArtifactRepo"
