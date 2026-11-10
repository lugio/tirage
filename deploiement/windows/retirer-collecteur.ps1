<#
.SYNOPSIS
  Retire le service collecteur tirage. PowerShell ADMINISTRATEUR.

.DESCRIPTION
  Arrête et supprime le service, puis le programme. La configuration (avec le jeton) et la file des travaux pas encore
  envoyés restent dans C:\ProgramData\tirage, sauf avec -ToutEffacer.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\retirer-collecteur.ps1
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\retirer-collecteur.ps1 -ToutEffacer
#>
param([switch]$ToutEffacer)
$ErrorActionPreference = 'Stop'
$prog = Join-Path $env:ProgramFiles 'tirage'
$dossier = Join-Path $env:ProgramData 'tirage'
$exe = Join-Path $prog 'tirage-collecteur.exe'
if (Get-Service tirage-collecteur -ErrorAction SilentlyContinue) {
    & $exe --desinstaller
    if ($LASTEXITCODE -ne 0) { throw 'Désinstallation du service refusée.' }
}
Remove-Item $prog -Recurse -Force -ErrorAction SilentlyContinue
if ($ToutEffacer) {
    Remove-Item $dossier -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'Service, programme, configuration et file effacés.'
} else {
    Write-Host "Service et programme retirés. Restent $dossier (configuration avec le jeton, file des travaux)."
}
