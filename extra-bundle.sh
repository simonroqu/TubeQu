# Von windows-bundle.sh eingebunden: PO-Token-Werkzeug neben tubequ.exe (TubeQu nimmt seinen Ordner in PATH auf).
cargo install --root target/botguard rustypipe-botguard && cp target/botguard/bin/rustypipe-botguard.exe "$out/bin/" ||
  echo "Hinweis: rustypipe-botguard nicht gebaut (Abspielen dann ohne Login)."
