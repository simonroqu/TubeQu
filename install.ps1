# Installiert die App unter Windows für den aktuellen Nutzer: MSYS2 (falls es fehlt), darin GTK4, libmpv und Rust,
# bauen, nach %LOCALAPPDATA%\Programs\<Titel> kopieren, Startmenü-Eintrag.
# Start: powershell -ExecutionPolicy Bypass -File install.ps1
# Entfernen: den Programmordner und den Startmenü-Eintrag löschen (stehen am Ende in der Ausgabe).
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$name = (Select-String -Path qu.toml -Pattern '^name = "(.*)"').Matches[0].Groups[1].Value
$title = (Select-String -Path qu.toml -Pattern '^title = "(.*)"').Matches[0].Groups[1].Value

$bash = 'C:\msys64\usr\bin\bash.exe'
if (-not (Test-Path $bash)) {
    winget install -e --id MSYS2.MSYS2 --accept-source-agreements --accept-package-agreements
    if (-not (Test-Path $bash)) { throw "MSYS2 nicht gefunden: $bash" }
}

if (Test-Path extra-deps.ps1) { . .\extra-deps.ps1 }

# UCRT64-Umgebung, im aktuellen Ordner bleiben.
$env:MSYSTEM = 'UCRT64'; $env:CHERE_INVOKING = '1'
& $bash -lc 'sh ./windows-bundle.sh'
if ($LASTEXITCODE) { throw 'Bauen fehlgeschlagen' }

$dest = "$env:LOCALAPPDATA\Programs\$title"
if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
Copy-Item -Recurse "target\$title" $dest
$lnkPath = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\$title.lnk"
$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($lnkPath)
$lnk.TargetPath = "$dest\bin\$name.exe"
$lnk.Save()
Write-Host "Installiert: $dest"
Write-Host "Startmenü:   $lnkPath"
