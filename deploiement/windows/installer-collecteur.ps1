<#
.SYNOPSIS
  Installe le collecteur tirage comme service Windows, sur le serveur d'impression.

.DESCRIPTION
  À lancer en PowerShell ADMINISTRATEUR, depuis le dossier de l'archive Windows d'une version.
  S'arrête à la première erreur ; rien n'est installé tant que les contrôles ne sont pas passés.
  - vérifie l'empreinte du programme d'après SHA256SUMS s'il est présent ;
  - vérifie que le serveur répond ;
  - crée C:\ProgramData\tirage, lisible par SYSTEM et les administrateurs seulement ;
  - demande le jeton du collecteur sans l'afficher et ne l'écrit que dans la configuration protégée ;
  - installe et démarre le service « tirage-collecteur ».
  Les copieurs à relever en SNMP s'ajoutent ensuite dans la configuration (voir docs/guides/installation.md).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\installer-collecteur.ps1 -Serveur https://tirage.example.org

.EXAMPLE
  # Certificat du serveur émis par une autorité que Windows ne connaît pas
  .\installer-collecteur.ps1 -Serveur https://192.0.2.20 -Autorite .\autorite.pem -Nom impression-1
#>
param(
    # Adresse du serveur tirage, en https.
    [Parameter(Mandatory)][ValidatePattern('^https://')][string]$Serveur,
    # Nom du collecteur, tel que déclaré côté serveur (tirage-serveur --nouveau-jeton NOM).
    [string]$Nom = $env:COMPUTERNAME.ToLower(),
    # Certificat de l'autorité qui a signé celui du serveur, si Windows ne la connaît pas.
    [string]$Autorite,
    # Titre des documents gardé : non, application (défaut) ou complet. Voir docs/architecture/0002-donnees-personnelles.md.
    [ValidateSet('non', 'application', 'complet')][string]$NomDocument = 'application'
)
$ErrorActionPreference = 'Stop'
$ici = Split-Path -Parent $MyInvocation.MyCommand.Path
$source = Join-Path $ici 'tirage-collecteur.exe'
$prog = Join-Path $env:ProgramFiles 'tirage'
$dossier = Join-Path $env:ProgramData 'tirage'
$conf = Join-Path $dossier 'collecteur.toml'
$exe = Join-Path $prog 'tirage-collecteur.exe'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Lancer ce script dans un PowerShell administrateur.'
}
if (Get-Service tirage-collecteur -ErrorAction SilentlyContinue) {
    throw 'Le service existe déjà : lancer d''abord retirer-collecteur.ps1 (la configuration est gardée).'
}
if (-not (Test-Path $source)) { throw "Programme introuvable : $source" }

Write-Host '1. Empreinte du programme'
$sommes = Join-Path $ici 'SHA256SUMS'
$lu = (Get-FileHash $source -Algorithm SHA256).Hash.ToLower()
if (Test-Path $sommes) {
    $ligne = Get-Content $sommes | Where-Object { $_ -match '\s\*?tirage-collecteur\.exe$' } | Select-Object -First 1
    if (-not $ligne) { throw 'tirage-collecteur.exe absent de SHA256SUMS.' }
    if ($ligne.Split(' ')[0].ToLower() -ne $lu) { throw "Empreinte différente de SHA256SUMS : $lu" }
    Write-Host "   conforme ($lu)"
} else {
    Write-Host "   SHA256SUMS absent : comparer à la main avec la page de la version : $lu"
}

Write-Host "2. Accès au serveur ($Serveur)"
$u = [Uri]$Serveur
$port = if ($u.IsDefaultPort) { 443 } else { $u.Port }
$t = New-Object Net.Sockets.TcpClient
try { $t.Connect($u.Host, $port) } catch { throw "Serveur injoignable ($($u.Host):$port) : pare-feu ou adresse à vérifier. Rien n'est installé." } finally { $t.Close() }

Write-Host '3. Dossiers et droits (SYSTEM et administrateurs seulement)'
New-Item -ItemType Directory -Force $prog, $dossier | Out-Null
& icacls $dossier /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls a échoué.' }
Copy-Item $source $exe -Force
$ligneCa = ''
if ($Autorite) {
    Copy-Item $Autorite (Join-Path $dossier 'autorite.pem') -Force
    $ligneCa = "ca = '$(Join-Path $dossier 'autorite.pem')'"
}

Write-Host '4. Jeton du collecteur (saisie masquée)'
$s = Read-Host "Jeton de « $Nom »" -AsSecureString
$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($s)
try {
    $jeton = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b).Trim()
    if ($jeton -notmatch '^[0-9a-f]{64}$') { throw 'Le jeton attendu fait 64 caractères hexadécimaux. Rien n''est installé.' }
    $texte = @"
# Configuration du collecteur. Contient le jeton (et la communauté SNMP des copieurs) : ne pas la copier ailleurs.
collecteur = "$Nom"
donnees = '$(Join-Path $dossier 'donnees')'
nom_document = "$NomDocument"

[serveur]
url = "$Serveur"
jeton = "$jeton"
$ligneCa

# Copieurs à relever en SNMP (lecture seule), voir docs/guides/installation.md :
# [snmp]
# intervalle_minutes = 60
#
# [[copieurs]]
# nom = "COPIEUR-1"
# adresse = "192.0.2.10"
# communaute = "public"
"@
    [IO.File]::WriteAllText($conf, ($texte -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b)
    Remove-Variable jeton, texte -ErrorAction SilentlyContinue
}

Write-Host '5. Service'
& $exe --installer --config $conf
if ($LASTEXITCODE -ne 0) { throw 'Installation du service refusée.' }
Start-Service tirage-collecteur
Start-Sleep -Seconds 10

Write-Host '6. État'
Get-Service tirage-collecteur | Format-Table Name, Status, StartType -AutoSize
Get-Content (Join-Path $dossier 'donnees\journal\collecteur.log') -Tail 15 -Encoding UTF8 -ErrorAction SilentlyContinue
Write-Host ''
Write-Host 'Fini. Imprimer une page : elle doit apparaître dans l''interface du serveur dans la minute.'
