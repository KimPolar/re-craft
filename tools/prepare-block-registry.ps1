param(
    [string]$MinecraftDir = (Join-Path $env:APPDATA '.minecraft'),
    [string]$JavaHome = $(if ($env:MINECRAFT_JAVA_HOME) { $env:MINECRAFT_JAVA_HOME } elseif ($env:JAVA_HOME) { $env:JAVA_HOME } else { Join-Path $env:USERPROFILE 'AppData\Local\Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime\java-runtime-epsilon\windows-x64\java-runtime-epsilon' }),
    [string]$OutputDirectory = (Join-Path (Split-Path -Parent $PSScriptRoot) '.local\block-registry')
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$versionDirectory = Join-Path $MinecraftDir 'versions\26.3'
$manifestPath = Join-Path $versionDirectory '26.3.json'
$jarPath = Join-Path $versionDirectory '26.3.jar'
$javaExe = Join-Path $JavaHome 'bin\java.exe'
if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf) -or !(Test-Path -LiteralPath $jarPath -PathType Leaf)) {
    throw 'Minecraft 26.3 metadata or client JAR is missing. Set -MinecraftDir to the Minecraft installation.'
}
if (!(Test-Path -LiteralPath $javaExe -PathType Leaf)) {
    throw 'Java 25 was not found. Set -JavaHome or MINECRAFT_JAVA_HOME to a Java 25 installation.'
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$clientSha1 = (Get-FileHash -LiteralPath $jarPath -Algorithm SHA1).Hash.ToLowerInvariant()
if ($manifest.id -ne '26.3' -or $manifest.downloads.client.sha1.ToLowerInvariant() -ne $clientSha1) {
    throw 'Minecraft client does not match its 26.3 metadata and SHA-1 checksum.'
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$jar = [IO.Compression.ZipFile]::OpenRead($jarPath)
try {
    $entry = $jar.GetEntry('version.json')
    if ($null -eq $entry) { throw 'The client JAR is missing version.json.' }
    $reader = [IO.StreamReader]::new($entry.Open())
    try { $version = $reader.ReadToEnd() | ConvertFrom-Json } finally { $reader.Dispose() }
} finally { $jar.Dispose() }
if ($version.id -ne '26.3' -or $version.world_version -ne 5023) {
    throw 'The client JAR is not Minecraft 26.3 with world data version 5023.'
}
$oldPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$javaVersion = (& $javaExe -version 2>&1 | Out-String)
$versionExit = $LASTEXITCODE
$ErrorActionPreference = $oldPreference
if ($versionExit -ne 0 -or $javaVersion -notmatch 'version "25(?:[.\-+"_]|$)') {
    throw 'The selected runtime is not Java 25. Set -JavaHome to Java 25.'
}
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $outputPath) {
    $existingMetadata = Join-Path $outputPath 'metadata.json'
    $existingReport = Join-Path $outputPath 'blocks.json'
    if ((Test-Path -LiteralPath $existingMetadata -PathType Leaf) -and (Test-Path -LiteralPath $existingReport -PathType Leaf)) {
        try {
            $cached = Get-Content -LiteralPath $existingMetadata -Raw | ConvertFrom-Json
            $existingSha1 = (Get-FileHash -LiteralPath $existingReport -Algorithm SHA1).Hash.ToLowerInvariant()
            if ($cached.report_format -eq 1 -and $cached.game_version -eq '26.3' -and $cached.data_version -eq 5023 -and
                $cached.client_sha1 -eq $clientSha1 -and $cached.report_sha1 -eq $existingSha1) {
                Write-Host "Block registry report is already prepared at: $outputPath"
                return
            }
        } catch {
            # Keep an existing cache untouched if it cannot be verified.
        }
    }
    throw "Output already exists but could not be verified: $outputPath. Choose another -OutputDirectory."
}
$classpath = @($jarPath)
foreach ($library in $manifest.libraries) {
    if ($library.downloads.artifact.path) {
        $libraryPath = Join-Path (Join-Path $MinecraftDir 'libraries') $library.downloads.artifact.path
        if (Test-Path -LiteralPath $libraryPath -PathType Leaf) { $classpath += $libraryPath }
    }
}
$workRoot = Join-Path $projectRoot '.local'
New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
$tempPath = Join-Path $workRoot ('block-report-work-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempPath | Out-Null
try {
    & $javaExe -cp ($classpath -join [IO.Path]::PathSeparator) net.minecraft.data.Main --reports --output $tempPath
    if ($LASTEXITCODE -ne 0) { throw 'Minecraft data report generation failed.' }
    $reportPath = Join-Path $tempPath 'reports\blocks.json'
    if (!(Test-Path -LiteralPath $reportPath -PathType Leaf)) { throw 'Minecraft did not produce reports/blocks.json.' }
    $reportSha1 = (Get-FileHash -LiteralPath $reportPath -Algorithm SHA1).Hash.ToLowerInvariant()
    New-Item -ItemType Directory -Path $outputPath | Out-Null
    Copy-Item -LiteralPath $reportPath -Destination (Join-Path $outputPath 'blocks.json')
    $metadata = @{
        report_format = 1
        game_version = '26.3'
        data_version = 5023
        client_sha1 = $clientSha1
        report_sha1 = $reportSha1
    }
    [IO.File]::WriteAllText((Join-Path $outputPath 'metadata.json'), ($metadata | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    Write-Host "Block registry report prepared at: $outputPath"
} finally {
    if (Test-Path -LiteralPath $tempPath) {
        $resolvedTemp = (Resolve-Path -LiteralPath $tempPath).Path
        $resolvedRoot = (Resolve-Path -LiteralPath $workRoot).Path
        if ($resolvedTemp -ne [IO.Path]::GetFullPath($tempPath) -or (Split-Path -Parent $resolvedTemp) -ne $resolvedRoot) {
            throw "Refusing to remove an unexpected temporary path: $resolvedTemp"
        }
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
}
