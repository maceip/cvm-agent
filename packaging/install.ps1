param(
  [string]$From = "",
  [string]$InstallDir = "$env:LOCALAPPDATA\llmattest\bin",
  [string]$Version = $(if ($env:LLMATTEST_VERSION) { $env:LLMATTEST_VERSION } else { "latest" }),
  [string]$ReleaseBaseUrl = $env:LLMATTEST_RELEASE_BASE_URL
)

$ErrorActionPreference = "Stop"

function Fail($Message) {
  Write-Error $Message
  exit 1
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("llmattest-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  if ($From) {
    $archive = Resolve-Path $From
  } else {
    if (-not $ReleaseBaseUrl) {
      Fail "LLMATTEST_RELEASE_BASE_URL is required when -From is not used."
    }
    $archive = Join-Path $tmp "llmattest.zip"
    $url = ($ReleaseBaseUrl.TrimEnd("/")) + "/llmattest-$Version-x86_64-pc-windows-msvc.zip"
    Invoke-WebRequest -Uri $url -OutFile $archive
  }

  $unpack = Join-Path $tmp "unpack"
  New-Item -ItemType Directory -Path $unpack | Out-Null
  Expand-Archive -Path $archive -DestinationPath $unpack -Force
  $bin = Get-ChildItem -Path $unpack -Recurse -Filter "llmattest.exe" | Select-Object -First 1
  if (-not $bin) {
    Fail "Archive did not contain llmattest.exe."
  }

  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  Copy-Item -Path $bin.FullName -Destination (Join-Path $InstallDir "llmattest.exe") -Force

  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if (-not ($userPath.Split(";") -contains $InstallDir)) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Host "Added $InstallDir to the user PATH. Open a new terminal to use it."
  }

  Write-Host "llmattest installed to $(Join-Path $InstallDir "llmattest.exe")"
} finally {
  Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
