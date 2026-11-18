# Von install.ps1 eingebunden: TubeQu spielt über yt-dlp, das für YouTube eine JS-Laufzeit (deno) braucht.
foreach ($id in 'yt-dlp.yt-dlp', 'DenoLand.Deno') {
    winget install -e --id $id --accept-source-agreements --accept-package-agreements
}
# yt-dlp-Plugin für den PO-Token (Abspielen mit Login gegen die Bot-Sperre); rustypipe-botguard.exe baut
# windows-bundle.sh und legt es neben tubequ.exe.
$plugin = 'yt_dlp_get_pot_rustypipe-0.2.0-py3-none-any.whl'
$plugins = "$env:APPDATA\yt-dlp\plugins"
if (-not (Test-Path "$plugins\$plugin")) {
    New-Item -ItemType Directory -Force $plugins | Out-Null
    try { Invoke-WebRequest -UseBasicParsing -OutFile "$plugins\$plugin" "https://files.pythonhosted.org/packages/py3/y/yt-dlp-get-pot-rustypipe/$plugin" }
    catch { Write-Host "Hinweis: yt-dlp-Plugin nicht geladen (Abspielen dann ohne Login)." }
}
