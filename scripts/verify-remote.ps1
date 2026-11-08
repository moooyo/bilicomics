param(
    [string]$RemoteRoot = '',
    [string]$RemoteTargetDirectory = '',
    [ValidateSet('core', 'defaults', 'pointer', 'reader', 'guest', 'startup', 'lifecycle', 'launcher', 'package')]
    [string[]]$Phases = @('core', 'defaults', 'pointer', 'reader', 'guest', 'startup', 'lifecycle', 'launcher'),
    [string]$EvidenceDirectory = '',
    [switch]$Kindle,
    [switch]$CheckOnly,
    [ValidateRange(1, 2147483647)]
    [int]$CargoBuildJobs = 12
)

$ErrorActionPreference = 'Stop'
$connectionOptions = @('-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15', '-o', 'ServerAliveInterval=20', '-o', 'ServerAliveCountMax=3')
if ($CheckOnly -and $Kindle) {
    throw 'CheckOnly cannot be combined with Kindle.'
}
if ([string]::IsNullOrWhiteSpace($RemoteRoot)) {
    $runIdentity = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $RemoteRoot = '/root/bilicomics-verification-' + $runIdentity
}
if ($RemoteRoot -notmatch '^/[A-Za-z0-9_./-]+$' -or $RemoteRoot.Contains('..') -or $RemoteRoot -eq '/') {
    throw 'RemoteRoot must be a simple absolute path without parent traversal.'
}
if ($RemoteTargetDirectory -and ($RemoteTargetDirectory -notmatch '^/[A-Za-z0-9_./-]+$' -or $RemoteTargetDirectory.Contains('..') -or $RemoteTargetDirectory -eq '/')) {
    throw 'RemoteTargetDirectory must be a simple absolute path without parent traversal.'
}
$projectRoot = Split-Path -Parent $PSScriptRoot
$runName = ($RemoteRoot.TrimEnd('/') -split '/')[-1]
if ([string]::IsNullOrWhiteSpace($EvidenceDirectory)) {
    $EvidenceDirectory = Join-Path $projectRoot ('artifacts\verification\remote-runs\' + $runName)
}
$EvidenceDirectory = [System.IO.Path]::GetFullPath($EvidenceDirectory)
if (Test-Path -LiteralPath $EvidenceDirectory) {
    throw 'The local evidence directory already exists; choose a fresh directory.'
}
$archivePath = Join-Path ([System.IO.Path]::GetTempPath()) ('bilicomics-source-' + [guid]::NewGuid().ToString('N') + '.tar.gz')
$remotePrepared = $false
try {
    & tar -czf $archivePath -C $projectRoot --exclude=.git --exclude=target --exclude=dist --exclude=artifacts --exclude=.secrets --exclude=.design-reference --exclude=.remote-staging --exclude=data --exclude=demo-data --exclude=__pycache__ --exclude='*.pyc' .
    if ($LASTEXITCODE -ne 0) { throw 'Source archive creation failed.' }
    & ssh @connectionOptions test-env "test ! -e '$RemoteRoot' && mkdir -m 700 -p '$RemoteRoot/app'"
    if ($LASTEXITCODE -ne 0) { throw 'The remote environment is unavailable or RemoteRoot already exists. Existing sources are never overlaid.' }
    $remotePrepared = $true
    & scp @connectionOptions $archivePath "test-env:$RemoteRoot/source.tar.gz"
    if ($LASTEXITCODE -ne 0) { throw 'Source transfer failed.' }
    & ssh @connectionOptions test-env "tar -xzf '$RemoteRoot/source.tar.gz' -C '$RemoteRoot/app'"
    if ($LASTEXITCODE -ne 0) { throw 'Remote source extraction failed.' }
    $kindleFlag = if ($Kindle) { '1' } else { '0' }
    $checkFlag = if ($CheckOnly) { '1' } else { '0' }
    $phaseList = ($Phases | Select-Object -Unique) -join ','
    $targetAssignment = if ($RemoteTargetDirectory) { "CARGO_TARGET_DIR='$RemoteTargetDirectory' " } else { '' }
    & ssh @connectionOptions test-env "CARGO_BUILD_JOBS=$CargoBuildJobs ${targetAssignment}sh '$RemoteRoot/app/scripts/check-remote.sh' '$RemoteRoot' '$kindleFlag' '$checkFlag' '$phaseList'"
    $verificationExit = $LASTEXITCODE
    New-Item -ItemType Directory -Force -Path $EvidenceDirectory | Out-Null
    & scp @connectionOptions -r "test-env:$RemoteRoot/evidence" $EvidenceDirectory
    if ($LASTEXITCODE -ne 0) { throw 'Remote evidence retrieval failed; preserve the remote workspace for diagnosis.' }
    if ($Kindle -and $verificationExit -eq 0) {
        & scp @connectionOptions "test-env:$RemoteRoot/app/artifacts/verification/bilicomics-kindle-*-dev.zip" $EvidenceDirectory
        if ($LASTEXITCODE -ne 0) { throw 'Development archive retrieval failed.' }
        & scp @connectionOptions "test-env:$RemoteRoot/app/artifacts/verification/bilicomics-kindle-*-dev.manifest.json" $EvidenceDirectory
        if ($LASTEXITCODE -ne 0) { throw 'Development manifest retrieval failed.' }
    }
    if ($verificationExit -ne 0) { throw "Remote verification failed; inspect $EvidenceDirectory and $RemoteRoot." }
    Write-Output "Remote verification evidence: $EvidenceDirectory"
} finally {
    if (Test-Path -LiteralPath $archivePath) { Remove-Item -LiteralPath $archivePath -Force }
    if ($remotePrepared) { Write-Output "Preserved remote workspace: $RemoteRoot" }
}
