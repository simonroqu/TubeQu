//! Login aufs Fernsehgerät übertragen. Android hat keine Browser-Cookies und Google sperrt eigene Anmeldungen
//! (siehe CLAUDE.md), darum schickt die Desktop-App ihren Browser-Login übers Heimnetz:
//! Der Fernseher zeigt einen Code „<IP-Endung>-<8 Ziffern>“ und wartet auf `PORT`; die Desktop-App sucht ihn in
//! ihren eigenen Netzen (gleiche ersten drei IP-Stellen) und koppelt sich mit dem Code.
//!
//! Verschlüsselt: Aus den 8 Ziffern handeln beide per SPAKE2 einen 256-Bit-Schlüssel aus (wer mitliest, kann den
//! Code nicht offline durchprobieren; ein Angreifer im Netz hat nur einen Rateversuch pro Verbindung, nach
//! `MAX_WRONG` ist zu). Der Login geht nur mit diesem Schlüssel verschlüsselt (ChaCha20-Poly1305) über die Leitung.
//! Feste Kopplung: Der Schlüssel bleibt gespeichert (TV `pair.key`, Desktop `tvs.tsv` mit IP), danach schickt der
//! Desktop neue Cookies ohne Code (`send_paired`); die TV-App wartet dafür, solange sie läuft (`receive(None)`).
//! ponytail: kein Schutz gegen Wiedereinspielen einer alten Nachricht (bringt nur ältere Cookies zurück).
//!
//! Ablauf (je eine JSON-Zeile): Code: Desktop `{"spake"}` → TV `{"spake"}` → Desktop `{"nonce","box"}` → TV „ok“/„wrong“.
//! Gekoppelt: Desktop `{"nonce","box"}` → TV „ok“/„wrong“.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Fester Port, auf dem der Fernseher wartet.
pub const PORT: u16 = 47821;
/// So viele falsche Codes, dann gibt der Fernseher auf.
const MAX_WRONG: u32 = 3;
/// Größte Verbindung (Cookies sind einige KB, als Hex doppelt).
const MAX_LEN: u64 = 256 * 1024;
/// Beide Seiten gleich (symmetrisches SPAKE2).
const ID: &[u8] = b"tubequ-pair-v1";

/// Was übertragen wird: Cookies (youtube.com/google.com) und das gewählte Google-Konto.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Login {
    pub cookies: Vec<(String, String)>,
    pub account: u32,
    pub name: String,
    pub email: String,
    pub photo: Option<String>,
}

impl Login {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "cookies": self.cookies,
            "account": self.account,
            "name": self.name,
            "email": self.email,
            "photo": self.photo,
        })
    }

    fn from_json(j: &serde_json::Value) -> Option<Login> {
        let cookies = j["cookies"]
            .as_array()?
            .iter()
            .filter_map(|c| Some((c[0].as_str()?.to_string(), c[1].as_str()?.to_string())))
            .collect::<Vec<_>>();
        cookies.iter().any(|(k, _)| k == "SAPISID").then_some(())?;
        Some(Login {
            cookies,
            account: j["account"].as_u64().unwrap_or(0) as u32,
            name: j["name"].as_str().unwrap_or_default().to_string(),
            email: j["email"].as_str().unwrap_or_default().to_string(),
            photo: j["photo"].as_str().map(String::from),
        })
    }
}

fn login_file() -> std::path::PathBuf {
    qu_core::app::data_file("login.json")
}

/// Übertragener Login (nur auf dem Fernseher/Handy vorhanden).
pub fn stored() -> Option<Login> {
    let text = std::fs::read_to_string(login_file()).ok()?;
    Login::from_json(&serde_json::from_str(&text).ok()?)
}

/// Abmelden: übertragenen Login löschen (die Kopplung bleibt).
pub fn forget() {
    let _ = std::fs::remove_file(login_file());
}

fn write(path: std::path::PathBuf, text: &str) {
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, text);
}

fn save(l: &Login) {
    write(login_file(), &l.to_json().to_string());
}

/// Fernseher: Schlüssel der festen Kopplung (Hex), bleibt beim Abmelden erhalten.
fn key_file() -> std::path::PathBuf {
    qu_core::app::data_file("pair.key")
}

/// Desktop: gekoppelte Fernseher, je Zeile „IP<TAB>Schlüssel (Hex)“.
fn tvs_file() -> std::path::PathBuf {
    qu_core::app::data_file("tvs.tsv")
}

/// Gekoppelte Fernseher mit gültigem Schlüssel (Einträge der unverschlüsselten Vorversion fallen raus).
fn tvs() -> Vec<(String, Vec<u8>)> {
    let text = std::fs::read_to_string(tvs_file()).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(ip, k)| Some((ip.to_string(), unhex(k).filter(|k| k.len() == 32)?)))
        .collect()
}

/// Desktop: gibt es schon einen gekoppelten Fernseher?
pub fn paired() -> bool {
    !tvs().is_empty()
}

fn remember_tv(ip: &str, key: &[u8]) {
    let mut list: Vec<_> = tvs().into_iter().filter(|(i, _)| i != ip).collect();
    list.push((ip.to_string(), key.to_vec()));
    write(tvs_file(), &list.iter().map(|(i, k)| format!("{i}\t{}\n", hex(k))).collect::<String>());
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Login verschlüsseln (zufällige Nonce je Nachricht).
fn seal(key: &[u8], l: &Login) -> serde_json::Value {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("Zufall vom Betriebssystem");
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("32-Byte-Schlüssel");
    let sealed = cipher.encrypt(&nonce.into(), l.to_json().to_string().as_bytes()).expect("Verschlüsseln");
    serde_json::json!({ "nonce": hex(&nonce), "box": hex(&sealed) })
}

/// Entschlüsseln und prüfen; falscher Schlüssel oder veränderte Nachricht → `None`.
fn open(key: &[u8], j: &serde_json::Value) -> Option<Login> {
    let nonce: [u8; 12] = unhex(j["nonce"].as_str()?)?.try_into().ok()?;
    let plain = ChaCha20Poly1305::new_from_slice(key).ok()?.decrypt(&nonce.into(), unhex(j["box"].as_str()?)?.as_slice()).ok()?;
    Login::from_json(&serde_json::from_slice(&plain).ok()?)
}

/// Eigene IP im Heimnetz (Fernseher): Absender-Adresse Richtung Internet, ohne etwas zu senden.
fn own_ip() -> Option<Ipv4Addr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// 8 Zufallsziffern (Zufall vom Betriebssystem).
fn secret() -> String {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("Zufall vom Betriebssystem");
    format!("{:08}", u64::from_le_bytes(b) % 100_000_000)
}

/// Fernseher: Code für den Bildschirm („112-4829-1374“) und das Geheimnis (die 8 Ziffern).
pub fn new_code() -> Result<(String, String), String> {
    let ip = own_ip().ok_or("Not connected to a network")?;
    let s = secret();
    Ok((format!("{}-{}-{}", ip.octets()[3], &s[..4], &s[4..]), s))
}

/// Halbe SPAKE2-Runde: eigene Nachricht und Zustand zum Abschließen.
fn spake(digits: &str) -> (Spake2<Ed25519Group>, Vec<u8>) {
    Spake2::<Ed25519Group>::start_symmetric(&Password::new(digits.as_bytes()), &Identity::new(ID))
}

/// Eine JSON-Zeile lesen (höchstens 10 s warten).
async fn read_json(r: &mut (impl AsyncBufRead + Unpin)) -> Option<serde_json::Value> {
    let mut line = String::new();
    match tokio::time::timeout(Duration::from_secs(10), r.read_line(&mut line)).await {
        Ok(Ok(n)) if n > 0 => serde_json::from_str(&line).ok().or(Some(serde_json::Value::String(line.trim().into()))),
        _ => None,
    }
}

async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), text: &str) -> bool {
    w.write_all(format!("{text}\n").as_bytes()).await.is_ok()
}

/// Port belegen; kurz warten, falls der vorige Empfänger (andere Seite) gerade erst abgebrochen wurde.
async fn bind() -> Result<tokio::net::TcpListener, String> {
    let mut last = String::new();
    for _ in 0..20 {
        match tokio::net::TcpListener::bind(("0.0.0.0", PORT)).await {
            Ok(l) => return Ok(l),
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("Couldn't wait for your computer: {last}"))
}

/// Fernseher: auf den Login warten. `secret` = angezeigte 8 Ziffern (höchstens `MAX_WRONG` Fehlversuche); ohne Code
/// nur Nachrichten mit dem Schlüssel der festen Kopplung (ohne Kopplung sofort Fehler). Speichert Login + Schlüssel
/// und stellt das Konto um. Abbrechen = die Future fallen lassen (Seite verlassen).
pub async fn receive(secret: Option<String>) -> Result<Login, String> {
    let key = std::fs::read_to_string(key_file()).ok().and_then(|k| unhex(&k)).filter(|k| k.len() == 32);
    if secret.is_none() && key.is_none() {
        return Err("Not paired with a computer".into());
    }
    let listener = bind().await?;
    let mut wrong = 0;
    loop {
        let (sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let (read, mut w) = sock.into_split();
        let mut r = BufReader::new(read.take(MAX_LEN));
        let Some(first) = read_json(&mut r).await else { continue };
        let got = match (&first["spake"], &secret) {
            // Code-Kopplung: Schlüssel aushandeln, dann den damit verschlüsselten Login lesen.
            (serde_json::Value::String(theirs), Some(digits)) => {
                let (state, mine) = spake(digits);
                if !write_line(&mut w, &serde_json::json!({ "spake": hex(&mine) }).to_string()).await {
                    continue;
                }
                let k = unhex(theirs).and_then(|t| state.finish(&t).ok());
                match (k, read_json(&mut r).await) {
                    (Some(k), Some(msg)) => open(&k, &msg).map(|l| (l, Some(k))),
                    _ => None,
                }
            }
            // Gekoppelt: direkt mit dem gespeicherten Schlüssel.
            _ => key.as_deref().and_then(|k| open(k, &first)).map(|l| (l, None)),
        };
        match got {
            Some((l, new_key)) => {
                let _ = write_line(&mut w, "ok").await;
                save(&l);
                if let Some(k) = new_key {
                    write(key_file(), &hex(&k));
                }
                crate::youtube::set_account(l.account);
                return Ok(l);
            }
            None => {
                let _ = write_line(&mut w, "wrong").await;
                // Nur Code-Versuche zählen: den 256-Bit-Schlüssel errät niemand.
                if first["spake"].is_string() && secret.is_some() {
                    wrong += 1;
                    if wrong >= MAX_WRONG {
                        return Err("Too many wrong codes. Start again.".into());
                    }
                }
            }
        }
    }
}

/// Code vom Bildschirm zerlegen: (IP-Endung, 8 Ziffern). Erwartet „112-4829-1374“ (Trenner beliebig).
fn parse_code(code: &str) -> Option<(u8, String)> {
    let groups: Vec<&str> = code.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()).collect();
    let octet = groups.first()?.parse().ok()?;
    let rest: String = groups[1..].concat();
    (rest.len() == 8).then_some((octet, rest))
}

/// Desktop: den Fernseher in den eigenen Netzen suchen (gleiche ersten drei Stellen + IP-Endung aus dem Code),
/// koppeln und den Login schicken. Fehlertext für die Oberfläche.
pub async fn send(code: String, login: Login) -> Result<(), String> {
    let (octet, digits) = parse_code(&code).ok_or("Type the code exactly as your TV shows it, e.g. 112-4829-1374")?;
    let candidates: Vec<SocketAddr> = if_addrs::get_if_addrs()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() => {
                let [a, b, c, _] = ip.octets();
                Some(SocketAddr::from((Ipv4Addr::new(a, b, c, octet), PORT)))
            }
            _ => None,
        })
        .collect();
    let mut tries = tokio::task::JoinSet::new();
    for addr in candidates {
        tries.spawn(async move { tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr)).await.ok()?.ok() });
    }
    let mut sock = None;
    while let Some(r) = tries.join_next().await {
        if let Ok(Some(s)) = r {
            sock = Some(s);
            break;
        }
    }
    let sock = sock.ok_or("TV not found. Is it on the same network and showing the code?")?;
    let ip = sock.peer_addr().map_err(|e| e.to_string())?.ip().to_string();
    let key = pair_with(sock, &digits, &login).await?;
    remember_tv(&ip, &key);
    Ok(())
}

/// Desktop-Seite der Code-Kopplung über eine offene Verbindung; Ergebnis = gemeinsamer Schlüssel.
async fn pair_with(sock: tokio::net::TcpStream, digits: &str, login: &Login) -> Result<Vec<u8>, String> {
    let (read, mut w) = sock.into_split();
    let mut r = BufReader::new(read.take(MAX_LEN));
    let (state, mine) = spake(digits);
    let no_answer = || "The TV didn't answer.".to_string();
    if !write_line(&mut w, &serde_json::json!({ "spake": hex(&mine) }).to_string()).await {
        return Err(no_answer());
    }
    let theirs = read_json(&mut r).await.ok_or_else(no_answer)?;
    let theirs = theirs["spake"].as_str().and_then(unhex).ok_or("The TV isn't showing a code. Open its sign-in page.")?;
    let key = state.finish(&theirs).map_err(|_| "Wrong code. Check the code on your TV.")?;
    write_line(&mut w, &seal(&key, login).to_string()).await;
    match read_json(&mut r).await.as_ref().and_then(|a| a.as_str()) {
        Some("ok") => Ok(key),
        Some("wrong") => Err("Wrong code. Check the code on your TV.".into()),
        _ => Err(no_answer()),
    }
}

/// Verschlüsselten Login über eine offene Verbindung schicken; Antwort „ok“/„wrong“ (leer = keine).
async fn push(sock: tokio::net::TcpStream, key: &[u8], login: &Login) -> String {
    let (read, mut w) = sock.into_split();
    if !write_line(&mut w, &seal(key, login).to_string()).await {
        return String::new();
    }
    let answer = read_json(&mut BufReader::new(read.take(MAX_LEN))).await;
    answer.as_ref().and_then(|a| a.as_str()).unwrap_or_default().to_string()
}

/// Desktop: neuen Login ohne Code an alle gekoppelten Fernseher (die TV-App muss laufen). Ok, wenn einer ihn nahm.
pub async fn send_paired(login: Login) -> Result<(), String> {
    let mut ok = false;
    for (ip, key) in tvs() {
        let Ok(addr) = format!("{ip}:{PORT}").parse::<SocketAddr>() else { continue };
        let Ok(Ok(sock)) = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr)).await else { continue };
        ok |= push(sock, &key, &login).await == "ok";
    }
    if ok { Ok(()) } else { Err("TV not reachable. Open TubeQu on your TV, or enter a new code.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(parse_code("112-4829-1374"), Some((112, "48291374".into())));
        assert_eq!(parse_code("112 48291374").map(|c| c.0), Some(112));
        assert_eq!(parse_code("112-4829"), None, "zu kurz");
        assert_eq!(parse_code("abc"), None);
        assert_eq!(secret().len(), 8);
        assert_ne!(secret(), secret());
        assert_eq!(unhex(&hex(&[0, 15, 255])), Some(vec![0, 15, 255]));
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn login_json_and_box() {
        let l = Login { cookies: vec![("SAPISID".into(), "x".into())], account: 2, name: "A".into(), email: "a@b".into(), photo: None };
        assert_eq!(Login::from_json(&l.to_json()), Some(l.clone()));
        // Ohne SAPISID kein gültiger Login.
        assert_eq!(Login::from_json(&serde_json::json!({ "cookies": [["SID", "y"]] })), None);
        // Verschlüsselt: Cookies nicht lesbar, nur mit dem richtigen Schlüssel zu öffnen, Veränderung fällt auf.
        let k = [7u8; 32];
        let mut b = seal(&k, &l);
        assert!(!b.to_string().contains("SAPISID"));
        assert_eq!(open(&k, &b), Some(l));
        assert_eq!(open(&[8u8; 32], &b), None);
        let mut bytes = unhex(b["box"].as_str().unwrap()).unwrap();
        bytes[0] ^= 1;
        b["box"] = hex(&bytes).into();
        assert_eq!(open(&k, &b), None);
    }

    /// Ganzer Ablauf über localhost: falscher Code, richtiger Code, dann gekoppelt ohne Code.
    #[tokio::test]
    async fn over_network() {
        let dir = std::env::temp_dir().join(format!("tubequ-pair-{}", std::process::id()));
        qu_core::app::set_dirs(dir.clone(), dir.clone());
        let l = Login { cookies: vec![("SAPISID".into(), "x".into())], account: 1, name: "A".into(), email: String::new(), photo: None };
        let tv_conn = || async { tokio::net::TcpStream::connect(("127.0.0.1", PORT)).await.unwrap() };

        let tv = tokio::spawn(receive(Some("48291374".into())));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(pair_with(tv_conn().await, "00000000", &l).await.is_err());
        let key = pair_with(tv_conn().await, "48291374", &l).await.unwrap();
        assert_eq!(tv.await.unwrap(), Ok(l.clone()));
        assert_eq!(stored(), Some(l.clone()));
        assert_eq!(std::fs::read_to_string(key_file()).ok().and_then(|k| unhex(&k)), Some(key.clone()));
        forget();

        // Gekoppelt: ohne Code nur mit dem ausgehandelten Schlüssel, falsche zählen nicht.
        let tv = tokio::spawn(receive(None));
        tokio::time::sleep(Duration::from_millis(100)).await;
        for _ in 0..MAX_WRONG {
            assert_eq!(push(tv_conn().await, &[1u8; 32], &l).await, "wrong");
        }
        assert_eq!(push(tv_conn().await, &key, &l).await, "ok");
        assert_eq!(tv.await.unwrap(), Ok(l.clone()));
        forget();

        // Desktop-Liste: gleiche IP wird ersetzt, alte Einträge ohne gültigen Schlüssel zählen nicht.
        write(tvs_file(), "10.0.0.9\t0123456789abcdef0123456789abcdef\n");
        assert!(!paired());
        remember_tv("10.0.0.5", &[1u8; 32]);
        remember_tv("10.0.0.5", &[2u8; 32]);
        assert_eq!(tvs(), [("10.0.0.5".to_string(), vec![2u8; 32])]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
