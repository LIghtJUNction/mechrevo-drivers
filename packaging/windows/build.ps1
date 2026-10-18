#requires -Version 7.4
<#
.SYNOPSIS
Build a fixed-identity Windows x64 MSI; never install or launch a control helper.
.DESCRIPTION
Run from a native x64 VS Build Tools developer PowerShell with Rust's
x86_64-pc-windows-msvc target, Windows SDK cl/mt/signtool, .NET 8+ and WiX 5.0.2.
Install build dependencies separately; this script does not download tools.
The locked Cargo dependencies must already be cached for --offline. The x64
MSVC developer prompt must provide the compiler, headers and import libraries;
an ordinary PowerShell with only cl.exe copied onto PATH is insufficient.
WiX 5.0.2: https://github.com/wixtoolset/wix/releases/tag/v5.0.2
WiX targets net6 with Major roll-forward, so a .NET 8 runtime can host it:
https://github.com/wixtoolset/wix/blob/v5.0.2/src/wix/wix/wix.csproj

-Candidate enables the explicit CLI/GUI windows-fnlock-candidate build feature.
It is not a runtime allowlist/path override or Windows hardware acceptance.
The default development_candidate trust is unsigned and must remain labelled as
such. Authenticode signs final executable bytes before hashing the release
manifest. The fixed support catalog is compiled into the Rust binaries.

MSI builds, upgrades, removal/repair and real ACL/UAC/Fn behavior require separate
Windows acceptance. A successful package build is not an installation receipt.
.EXAMPLE
pwsh -NoProfile -File .\packaging\windows\build.ps1 -MsiVersion 0.1.172 `
  -ReleaseId console-0.1.172-candidate -Candidate -OutputDirectory C:\Build\console-172
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')][string]$MsiVersion,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9._-]{1,128}$')][string]$ReleaseId,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [ValidateSet('development_candidate', 'authenticode')][string]$Trust = 'development_candidate',
    [switch]$Candidate,
    [ValidatePattern('^[0-9A-Fa-f]{40}$')][string]$SigningCertificateThumbprint,
    [uri]$TimestampUrl
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows -or -not [Environment]::Is64BitProcess -or
    [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64) {
    throw 'Build only in native Windows x64 PowerShell 7.4+; cross-compiled MSI acceptance is not assumed.'
}
$parts = $MsiVersion.Split('.') | ForEach-Object { [uint64]::Parse($_) }
if ($parts[0] -gt 255 -or $parts[1] -gt 255 -or $parts[2] -gt 65535) { throw 'MSI version must fit 255.255.65535.' }
if ($Trust -eq 'authenticode') {
    if (-not $SigningCertificateThumbprint -or -not $TimestampUrl -or $TimestampUrl.Scheme -ne 'https' -or
        $TimestampUrl.UserInfo -or $TimestampUrl.Fragment) {
        throw 'Authenticode requires an explicit certificate thumbprint and HTTPS RFC3161 TimestampUrl.'
    }
} elseif ($SigningCertificateThumbprint -or $TimestampUrl) {
    throw 'Signing options require Trust=authenticode; do not silently downgrade signature expectations.'
}
function Resolve-Tool([string]$Name) {
    return (Get-Command -Name $Name -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
}
$git = Resolve-Tool 'git.exe'
$cargo = Resolve-Tool 'cargo.exe'
$cl = Resolve-Tool 'cl.exe'
$mt = Resolve-Tool 'mt.exe'
$wix = Resolve-Tool 'wix.exe'
$dotnet = Resolve-Tool 'dotnet.exe'
$signtool = if ($Trust -eq 'authenticode') { Resolve-Tool 'signtool.exe' } else { $null }
$wixVersion = (& $wix --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $wixVersion -notmatch '^5\.0\.2(?:[.+]|$)') { throw "Require WiX 5.0.2, got: $wixVersion" }
$runtimes = & $dotnet --list-runtimes
if ($LASTEXITCODE -ne 0 -or -not ($runtimes | Where-Object { $_ -match '^Microsoft\.NETCore\.App (?:[89]|[1-9][0-9])\.' })) {
    throw 'A .NET 8 or newer runtime is required for this build toolchain.'
}
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$sourceCommit = (& $git -C $repo rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $sourceCommit -cnotmatch '^[0-9a-f]{40}$') { throw 'Cannot freeze the Git source commit.' }
$dirty = & $git -C $repo status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Commit all source/package inputs before building this release.' }
$out = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $out) { throw 'OutputDirectory must be new; never overwrite an existing release.' }
if ($env:RUSTFLAGS -or $env:CARGO_ENCODED_RUSTFLAGS) { throw 'Unset caller Rust flags; this recipe selects the static MSVC CRT explicitly.' }
$resources = Join-Path $repo 'console\resources\windows'
foreach ($required in @('console-cli.manifest', 'console-gui.manifest', 'console-oem-helper.manifest', 'installer-guard.c', 'release-manifest.schema.json')) {
    $item = Get-Item -LiteralPath (Join-Path $resources $required) -Force
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Invalid tracked resource: $required" }
    & $git -C $repo ls-files --error-unmatch "console/resources/windows/$required" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Untracked release resource: $required" }
}
$catalog = Get-Item -LiteralPath (Join-Path $repo 'console\resources\windows-fnlock-support.json') -Force
if ($catalog.PSIsContainer -or ($catalog.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid compiled-in support catalog.' }
& $git -C $repo ls-files --error-unmatch 'console/resources/windows-fnlock-support.json' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'The compiled-in support catalog must be tracked.' }
$payload = Join-Path $out 'payload'
$logs = Join-Path $out 'logs'
$intermediate = Join-Path $out 'intermediate'
$target = Join-Path $out 'cargo-target'
$temporary = Join-Path $out 'tmp'
New-Item -ItemType Directory -Path $payload, $logs, $intermediate, $target, $temporary -Force | Out-Null
$commands = [Collections.Generic.List[object]]::new()
$savedEnvironment = @{}
foreach ($key in @('MECHREVO_WINDOWS_RELEASE_ID', 'MECHREVO_WINDOWS_SOURCE_COMMIT', 'CARGO_TARGET_DIR', 'CARGO_ENCODED_RUSTFLAGS', 'TEMP', 'TMP')) {
    $savedEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
$complete = $false
function Invoke-Checked([string]$Name, [string]$Executable, [string[]]$Arguments) {
    $log = Join-Path $logs "$Name.log"
    & $Executable @Arguments 2>&1 | Tee-Object -FilePath $log | Out-Host
    $code = $LASTEXITCODE
    $commands.Add([ordered]@{ name = $Name; exit_code = $code; log = [IO.Path]::GetFileName($log) })
    if ($code -ne 0) { throw "$Name failed with exit code $code; retain $log" }
}
function Assert-Amd64([string]$Path) {
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Not a PE file: $Path" }
        $stream.Position = 0x3c; $offset = $reader.ReadUInt32()
        if ($offset -gt ($stream.Length - 26)) { throw "Truncated PE: $Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw "Not AMD64 PE: $Path" }
    } finally { $stream.Dispose() }
}
function Sign-Artifact([string]$Name, [string]$Path) {
    Invoke-Checked "sign-$Name" $signtool @('sign', '/sha1', $SigningCertificateThumbprint, '/s', 'My', '/fd', 'SHA256', '/tr', $TimestampUrl.AbsoluteUri, '/td', 'SHA256', $Path)
    Invoke-Checked "verify-sign-$Name" $signtool @('verify', '/pa', '/all', '/v', $Path)
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode' -or
        $signature.SignerCertificate.Thumbprint -ine $SigningCertificateThumbprint) { throw "Unexpected signing identity: $Path" }
}
try {
    $env:MECHREVO_WINDOWS_RELEASE_ID = $ReleaseId
    $env:MECHREVO_WINDOWS_SOURCE_COMMIT = $sourceCommit
    $env:CARGO_TARGET_DIR = $target
    $env:CARGO_ENCODED_RUSTFLAGS = '-Ctarget-feature=+crt-static'
    $env:TEMP = $temporary
    $env:TMP = $temporary
    $features = 'mechrevo-console-gui/gui'
    if ($Candidate) { $features += ',mechrevo-console-cli/windows-fnlock-candidate,mechrevo-console-gui/windows-fnlock-candidate' }
    Invoke-Checked 'cargo-release' $cargo @('build', '--manifest-path', (Join-Path $repo 'console\Cargo.toml'), '--release', '--workspace', '--locked', '--offline', '--target', 'x86_64-pc-windows-msvc', '--features', $features)
    $artifactNames = @('mechrevo-console.exe', 'mechrevo-console-gui.exe', 'mechrevo-console-oem-helper.exe')
    $manifestNames = @('console-cli.manifest', 'console-gui.manifest', 'console-oem-helper.manifest')
    for ($i = 0; $i -lt $artifactNames.Count; $i++) {
        $dest = Join-Path $payload $artifactNames[$i]
        Copy-Item -LiteralPath (Join-Path $target "x86_64-pc-windows-msvc\release\$($artifactNames[$i])") -Destination $dest
        Assert-Amd64 $dest
        $manifest = Join-Path $resources $manifestNames[$i]
        Invoke-Checked "embed-manifest-$i" $mt @('-nologo', '-manifest', $manifest, "-outputresource:$dest;#1")
        $extracted = Join-Path $intermediate "manifest-$i.xml"
        Invoke-Checked "extract-manifest-$i" $mt @('-nologo', "-inputresource:$dest;#1", "-out:$extracted")
        [xml]$embedded = Get-Content -LiteralPath $extracted -Raw
        $nodes = $embedded.SelectNodes('//*[local-name()="requestedExecutionLevel"]')
        $requiredLevel = if ($i -eq 2) { 'requireAdministrator' } else { 'asInvoker' }
        if ($nodes.Count -ne 1 -or $nodes[0].GetAttribute('level') -cne $requiredLevel -or $nodes[0].GetAttribute('uiAccess') -cne 'false') { throw "Wrong embedded privilege manifest: $dest" }
        if ($Trust -eq 'authenticode') { Sign-Artifact "exe-$i" $dest }
    }
    $identityHeader = Join-Path $intermediate 'build-identity.h'
    [IO.File]::WriteAllText($identityHeader, "#define MECHREVO_WINDOWS_RELEASE_ID `"$ReleaseId`"`n#define MECHREVO_WINDOWS_SOURCE_COMMIT `"$sourceCommit`"`n", [Text.UTF8Encoding]::new($false))
    $guard = Join-Path $intermediate 'installer-guard.dll'
    Invoke-Checked 'compile-maintenance-guard' $cl @('/nologo', '/LD', '/MT', '/W4', '/WX', '/DUNICODE', '/D_UNICODE', "/FI$identityHeader", "/Fo$(Join-Path $intermediate 'installer-guard.obj')", (Join-Path $resources 'installer-guard.c'), '/link', '/MACHINE:X64', "/OUT:$guard", "/IMPLIB:$(Join-Path $intermediate 'installer-guard.lib')", 'msi.lib', 'advapi32.lib', 'bcrypt.lib', 'shell32.lib', 'ole32.lib', 'uuid.lib')
    Assert-Amd64 $guard
    if ($Trust -eq 'authenticode') { Sign-Artifact 'guard' $guard }
    $files = [ordered]@{}
    foreach ($name in $artifactNames) { $files[$name] = (Get-FileHash -LiteralPath (Join-Path $payload $name) -Algorithm SHA256).Hash.ToLowerInvariant() }
    $identity = [ordered]@{ schema_version = 1; release_id = $ReleaseId; source_commit = $sourceCommit; architecture = 'x86_64'; trust = $Trust; files = $files }
    [IO.File]::WriteAllText((Join-Path $payload 'console-release.json'), (($identity | ConvertTo-Json -Depth 8) + "`n"), [Text.UTF8Encoding]::new($false))
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $payload 'LICENSE.txt')
    foreach ($license in @('Manrope-OFL.txt', 'Cormorant-OFL.txt')) { Copy-Item -LiteralPath (Join-Path $repo "console\crates\gui\assets\fonts\$license") -Destination (Join-Path $payload $license) }
    $msi = Join-Path $out "mechrevo-console-$MsiVersion-x64.msi"
    Invoke-Checked 'wix-build' $wix @('build', (Join-Path $PSScriptRoot 'console.wxs'), '-arch', 'x64', '-wx', '-d', "PayloadDir=$payload", '-d', "GuardDll=$guard", '-d', "MsiVersion=$MsiVersion", '-intermediateFolder', (Join-Path $intermediate 'wix'), '-o', $msi)
    if ($Trust -eq 'authenticode') { Sign-Artifact 'msi' $msi }
    $after = (& $git -C $repo rev-parse HEAD | Out-String).Trim()
    $dirtyAfter = & $git -C $repo status --porcelain --untracked-files=normal
    if ($LASTEXITCODE -ne 0 -or $after -cne $sourceCommit -or $dirtyAfter) { throw 'Source changed during the package build; do not publish this MSI.' }
    $complete = $true
} finally {
    foreach ($key in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $savedEnvironment[$key], 'Process') }
    $receipt = [ordered]@{
        schema_version = 1; build_complete = $complete; source_commit = $sourceCommit; release_id = $ReleaseId
        msi_version = $MsiVersion; trust = $Trust; candidate_build_feature = [bool]$Candidate
        target = 'x86_64-pc-windows-msvc'; wix_version = $wixVersion; commands = $commands.ToArray()
        installed = $false; windows_runtime_verified = $false; hardware_setter_executed = $false
        scope = 'Build receipt only. No MSI installation, upgrade/removal/repair, ACL/UAC or hardware acceptance.'
    }
    if ($complete) {
        $receipt['msi_sha256'] = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
        $receipt['release_manifest_sha256'] = (Get-FileHash -LiteralPath (Join-Path $payload 'console-release.json') -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    [IO.File]::WriteAllText((Join-Path $out 'build-receipt.json'), (($receipt | ConvertTo-Json -Depth 12) + "`n"), [Text.UTF8Encoding]::new($false))
}
