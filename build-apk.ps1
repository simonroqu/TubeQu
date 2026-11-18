# Baut die Android-App (APK) unter Windows: über WSL (Linux in Windows) mit demselben build-apk.sh wie unter Linux.
# Start: powershell -ExecutionPolicy Bypass -File build-apk.ps1
# Beim ersten Mal ohne WSL: richtet WSL + Ubuntu ein (Administrator-Abfrage), danach Neustart, Ubuntu einmal öffnen
# (Benutzername + Passwort anlegen) und dieses Skript erneut starten. Ergebnis: target\<Titel>.apk
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$title = (Select-String -Path qu.toml -Pattern '^title = "(.*)"').Matches[0].Groups[1].Value

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    throw 'WSL fehlt: braucht Windows 10 (Version 2004 oder neuer) oder Windows 11.'
}
# Läuft eine Linux-Umgebung? Sonst WSL mit Ubuntu einrichten (braucht Administrator-Rechte und einen Neustart).
& wsl.exe -e true 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host 'WSL mit Ubuntu wird eingerichtet (Administrator-Abfrage) ...'
    Start-Process wsl.exe -ArgumentList '--install', '-d', 'Ubuntu' -Verb RunAs -Wait
    Write-Host ''
    Write-Host 'Fertig. Jetzt: Windows neu starten, "Ubuntu" aus dem Startmenue einmal oeffnen'
    Write-Host '(Benutzername + Passwort anlegen) und dieses Skript erneut starten.'
    exit 0
}

# Ordner in WSL-Schreibweise (C:\… → /mnt/c/…) und dort bauen.
# ponytail: baut direkt im Windows-Ordner (über /mnt/c langsamer als im Linux-Dateisystem); reicht für gelegentliche Builds.
$dir = (& wsl.exe -e wslpath -a ($PSScriptRoot -replace '\\', '/')).Trim()
& wsl.exe -e env NO_ADB=1 sh -c "cd '$dir' && sh ./build-apk.sh"
if ($LASTEXITCODE) { throw 'Bauen fehlgeschlagen' }

$apk = "$PSScriptRoot\target\$title.apk"
Write-Host "APK: $apk"
# Installieren mit dem adb von Windows (WSL sieht keine USB-Geräte), falls vorhanden und genau ein Gerät verbunden ist.
$adb = Get-Command adb.exe -ErrorAction SilentlyContinue
if ($adb) {
    $devices = @(& $adb.Source devices | Select-String "`tdevice$").Count
    if ($devices -eq 1) {
        & $adb.Source install -r $apk
        if (-not $LASTEXITCODE) { Write-Host 'Installiert auf dem verbundenen Geraet.' }
        exit 0
    }
}
Write-Host 'Installieren: die APK aufs Geraet kopieren und dort oeffnen, oder mit adb'
Write-Host "(winget install -e --id Google.PlatformTools, Geraet mit USB-Debugging anschliessen): adb install -r `"$apk`""
