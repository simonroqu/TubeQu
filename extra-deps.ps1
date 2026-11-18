# Von install.ps1 eingebunden: TubeQu spielt über yt-dlp, das für YouTube eine JS-Laufzeit (deno) braucht.
foreach ($id in 'yt-dlp.yt-dlp', 'DenoLand.Deno') {
    winget install -e --id $id --accept-source-agreements --accept-package-agreements
}
