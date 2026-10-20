param(
  [ValidateSet('Auto', 'Manual')]
  [string]$Mode
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ToolchainText = Get-Content (Join-Path $ProjectRoot 'rust-toolchain.toml') -Raw
$Toolchain = [regex]::Match($ToolchainText, 'channel\s*=\s*"([^"]+)"').Groups[1].Value
$Missing = [System.Collections.Generic.List[string]]::new()

function Show-ManualGuide {
  Write-Host ''
  Write-Host 'Guide manuel - préparez tout en une seule fois, dans cet ordre :' -ForegroundColor Cyan
  Write-Host '1. Installez Node.js 20 ou plus récent : https://nodejs.org/'
  Write-Host '2. Ouvrez une nouvelle fenêtre PowerShell et installez pnpm : npm install --global pnpm@10.13.1'
  Write-Host '3. Installez Rustup pour Windows : https://rustup.rs/'
  Write-Host "4. Dans PowerShell, installez la toolchain du projet : rustup toolchain install $Toolchain --component rustfmt --component rust-analyzer --component rust-src"
  Write-Host '5. Revenez à la racine du projet, puis lancez : pnpm install --frozen-lockfile'
  Write-Host '6. Téléchargez les dépendances Rust : cargo fetch --locked'
  Write-Host '7. Vérifiez les prérequis : pnpm run check:windows-prereqs'
  Write-Host '8. Démarrez FreeClaude Voice : pnpm dev'
  Write-Host ''
  Write-Host 'Python et cargo-watch ne sont pas requis. Si Rust signale un linker MSVC absent, installez les outils C++ de Visual Studio : https://rust-lang.github.io/rustup/installation/windows-msvc.html' -ForegroundColor DarkGray
  Write-Host 'Les installations automatiques peuvent demander une confirmation Windows (UAC).' -ForegroundColor DarkGray
}

function Refresh-Path {
  $MachinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = @($MachinePath, $UserPath) -join ';'
}

if (-not $Mode) {
  Write-Host 'Choisissez une seule méthode pour installer tous les prérequis :' -ForegroundColor Cyan
  Write-Host '  A - Installation automatique des outils et dépendances du projet'
  Write-Host '  M - Guide complet pour tout installer manuellement'
  do { $Choice = (Read-Host 'Votre choix [A/M]').Trim().ToUpperInvariant() } while ($Choice -notin @('A', 'M'))
  $Mode = if ($Choice -eq 'A') { 'Auto' } else { 'Manual' }
}

if ($Mode -eq 'Manual') {
  Show-ManualGuide
  exit 0
}

function Get-CommandVersion([string]$Name, [string[]]$Arguments) {
  $Command = Get-Command $Name -ErrorAction SilentlyContinue
  if (-not $Command) { return $null }
  try { return ((& $Command.Source @Arguments 2>$null | Out-String).Trim()) }
  catch { return $null }
}

$NodeVersion = Get-CommandVersion 'node' @('--version')
if ($NodeVersion -and $NodeVersion -match '^v?(\d+)') {
  if ([int]$Matches[1] -ge 20) { Write-Host "[OK] Node.js $NodeVersion" -ForegroundColor Green }
  else { Write-Host "[À installer] Node.js 20+ (présent : $NodeVersion)" -ForegroundColor Yellow; $Missing.Add('node') }
} else { Write-Host '[À installer] Node.js 20+' -ForegroundColor Yellow; $Missing.Add('node') }

$PnpmVersion = Get-CommandVersion 'pnpm' @('--version')
if ($PnpmVersion -and $PnpmVersion -match '^(\d+)') {
  if ([int]$Matches[1] -ge 8) { Write-Host "[OK] pnpm $PnpmVersion" -ForegroundColor Green }
  else { Write-Host "[À installer] pnpm 8+ (présent : $PnpmVersion)" -ForegroundColor Yellow; $Missing.Add('pnpm') }
} else { Write-Host '[À installer] pnpm 8+' -ForegroundColor Yellow; $Missing.Add('pnpm') }

$Rustup = Get-Command rustup -ErrorAction SilentlyContinue
$Toolchains = if ($Rustup) { & $Rustup.Source toolchain list 2>$null | Out-String } else { '' }
if ($Toolchains -match "(?m)^$([regex]::Escape($Toolchain))(?:-|\s|$)") {
  Write-Host "[OK] Rust $Toolchain" -ForegroundColor Green
} else {
  Write-Host "[À installer] Rust $Toolchain" -ForegroundColor Yellow
  $Missing.Add('rust')
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Host '[À installer] cargo (fourni par Rustup)' -ForegroundColor Yellow
  if (-not $Missing.Contains('rust')) { $Missing.Add('rust') }
}

Write-Host '[INFO] Python et cargo-watch ne sont pas nécessaires.' -ForegroundColor Cyan
if ($Missing.Count -gt 0 -and -not (Get-Command winget -ErrorAction SilentlyContinue)) {
  Write-Host 'WinGet est indisponible. Je vous affiche le guide manuel complet.' -ForegroundColor Yellow
  Show-ManualGuide
  exit 1
}

if ($Missing.Contains('node')) {
  & winget install --id OpenJS.NodeJS.LTS --exact --accept-source-agreements --accept-package-agreements
  if ($LASTEXITCODE -ne 0) { throw "Installation de Node.js échouée." }
}
if ($Missing.Contains('rust')) {
  & winget install --id Rustlang.Rustup --exact --source winget --accept-source-agreements --accept-package-agreements
  if ($LASTEXITCODE -ne 0) { throw "Installation de Rustup échouée." }
}
Refresh-Path

$Npm = Get-Command npm -ErrorAction SilentlyContinue
if (-not $Npm) {
  Write-Host 'Node.js est installé. Fermez puis rouvrez PowerShell et relancez pnpm run setup:windows.' -ForegroundColor Yellow
  exit 0
}
if ($Missing.Contains('pnpm')) {
  & $Npm.Source install --global pnpm@10.13.1
  if ($LASTEXITCODE -ne 0) { throw "Installation de pnpm échouée." }
}

$Rustup = Get-Command rustup -ErrorAction SilentlyContinue
if ($Rustup -and $Missing.Contains('rust')) {
  & $Rustup.Source toolchain install $Toolchain --component rustfmt --component rust-analyzer --component rust-src
  if ($LASTEXITCODE -ne 0) { throw "Installation de Rust $Toolchain échouée." }
}
Refresh-Path

$Pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
$Cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $Pnpm -or -not $Cargo) {
  Write-Host 'Les outils Windows sont installés. Fermez puis rouvrez PowerShell, puis relancez pnpm run setup:windows.' -ForegroundColor Yellow
  exit 0
}

Push-Location $ProjectRoot
try {
  & $Pnpm.Source install --frozen-lockfile
  if ($LASTEXITCODE -ne 0) { throw "Installation des dépendances JavaScript échouée." }
  & $Cargo.Source fetch --locked
  if ($LASTEXITCODE -ne 0) { throw 'Le téléchargement des dépendances Rust a échoué.' }
} finally {
  Pop-Location
}
Write-Host 'Installation terminée. Ouverture de FreeClaude Voice...' -ForegroundColor Green
& $Pnpm.Source dev
