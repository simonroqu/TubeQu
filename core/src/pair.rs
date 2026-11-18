//! Login aufs Fernsehgerät übertragen. Android hat keine Browser-Cookies und Google sperrt eigene Anmeldungen
//! (siehe CLAUDE.md), darum schickt die Desktop-App ihren Browser-Login einmal übers Heimnetz:
//! Der Fernseher zeigt einen Code „<IP-Endung>-<8 Ziffern>“ und wartet auf `PORT`; die Desktop-App sucht ihn in
//! ihren eigenen Netzen (gleiche ersten drei IP-Stellen) und schickt den Login mit dem Code.
//! Schutz: nur solange der Code angezeigt wird, nach dem ersten Erfolg zu, nach `MAX_WRONG` falschen Codes zu.
//! ponytail: unverschlüsselt im Heimnetz (wer dort mitliest, sieht die Cookies); TLS/Schlüsseltausch erst, wenn nötig.

use std::hash::{BuildHasher, Hasher};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Fester Port, auf dem der Fernseher wartet.
pub const PORT: u16 = 47821;
/// So viele falsche Codes, dann gibt der Fernseher auf.
const MAX_WRONG: u32 = 3;
/// Größte Nachricht (Cookies sind einige KB).
const MAX_LEN: u64 = 64 * 1024;

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
    fn to_json(&self, code: &str) -> serde_json::Value {
        serde_json::json!({
            "code": code,
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

/// Abmelden: übertragenen Login löschen.
pub fn forget() {
    let _ = std::fs::remove_file(login_file());
}

fn save(l: &Login) {
    let path = login_file();
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, l.to_json("").to_string());
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

/// 8 Zufallsziffern. ponytail: Zufall aus `RandomState` (vom Betriebssystem geseedet) statt eigener rand-Abhängigkeit.
fn secret() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64);
    format!("{:08}", h.finish() % 100_000_000)
}

/// Fernseher: Code für den Bildschirm („112-4829-1374“) und das Geheimnis zum Prüfen.
pub fn new_code() -> Result<(String, String), String> {
    let ip = own_ip().ok_or("Not connected to a network")?;
    let s = secret();
    Ok((format!("{}-{}-{}", ip.octets()[3], &s[..4], &s[4..]), s))
}

/// Gleich lange Texte vergleichen, ohne beim ersten Unterschied abzubrechen.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Fernseher: auf den Login warten (bis zum ersten richtigen Code, höchstens `MAX_WRONG` falsche).
/// Speichert ihn und stellt das Konto um. Abbrechen = die Future fallen lassen (Seite verlassen).
pub async fn receive(secret: String) -> Result<Login, String> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", PORT)).await.map_err(|e| format!("Couldn't wait for your computer: {e}"))?;
    let mut wrong = 0;
    loop {
        let (sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let (read, mut write) = sock.into_split();
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(10), BufReader::new(read.take(MAX_LEN)).read_line(&mut line)).await;
        if !matches!(n, Ok(Ok(len)) if len > 0) {
            continue;
        }
        let j: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let code: String = j["code"].as_str().unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
        // Der Code enthält vorne die IP-Endung; geprüft werden die letzten 8 Ziffern.
        let given = code.get(code.len().saturating_sub(8)..).unwrap_or_default();
        match Login::from_json(&j).filter(|_| same(given, &secret)) {
            Some(l) => {
                let _ = write.write_all(b"ok\n").await;
                save(&l);
                crate::youtube::set_account(l.account);
                return Ok(l);
            }
            None => {
                let _ = write.write_all(b"wrong\n").await;
                wrong += 1;
                if wrong >= MAX_WRONG {
                    return Err("Too many wrong codes. Start again.".into());
                }
            }
        }
    }
}

/// Code vom Bildschirm zerlegen: (IP-Endung, Code wie getippt). Erwartet „112-4829-1374“ (Trenner beliebig).
fn parse_code(code: &str) -> Option<(u8, String)> {
    let groups: Vec<&str> = code.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()).collect();
    let octet = groups.first()?.parse().ok()?;
    let rest: String = groups[1..].concat();
    (rest.len() == 8).then(|| (octet, code.to_string()))
}

/// Desktop: den Fernseher in den eigenen Netzen suchen (gleiche ersten drei Stellen + IP-Endung aus dem Code)
/// und den Login schicken. Fehlertext für die Oberfläche.
pub async fn send(code: String, login: Login) -> Result<(), String> {
    let (octet, code) = parse_code(&code).ok_or("Type the code exactly as your TV shows it, e.g. 112-4829-1374")?;
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
    let (read, mut write) = sock.into_split();
    write.write_all(format!("{}\n", login.to_json(&code)).as_bytes()).await.map_err(|e| e.to_string())?;
    let mut answer = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), BufReader::new(read).read_line(&mut answer)).await;
    match answer.trim() {
        "ok" => Ok(()),
        "wrong" => Err("Wrong code. Check the code on your TV.".into()),
        _ => Err("The TV didn't answer.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(parse_code("112-4829-1374"), Some((112, "112-4829-1374".into())));
        assert_eq!(parse_code("112 48291374").map(|c| c.0), Some(112));
        assert_eq!(parse_code("112-4829"), None, "zu kurz");
        assert_eq!(parse_code("abc"), None);
        assert!(same("48291374", "48291374") && !same("48291374", "48291375") && !same("1", "12"));
        assert_eq!(secret().len(), 8);
    }

    #[test]
    fn login_json() {
        let l = Login { cookies: vec![("SAPISID".into(), "x".into())], account: 2, name: "A".into(), email: "a@b".into(), photo: None };
        assert_eq!(Login::from_json(&l.to_json("1")), Some(l));
        // Ohne SAPISID kein gültiger Login.
        assert_eq!(Login::from_json(&serde_json::json!({ "cookies": [["SID", "y"]] })), None);
    }

    /// Ganzer Ablauf über localhost: falscher Code, dann richtiger.
    #[tokio::test]
    async fn over_network() {
        let dir = std::env::temp_dir().join(format!("tubequ-pair-{}", std::process::id()));
        qu_core::app::set_dirs(dir.clone(), dir.clone());
        let tv = tokio::spawn(receive("48291374".into()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let l = Login { cookies: vec![("SAPISID".into(), "x".into())], account: 1, name: "A".into(), email: String::new(), photo: None };
        // send() sucht in den echten Netzen; hier direkt an localhost.
        let talk = |code: &'static str, l: Login| async move {
            let s = tokio::net::TcpStream::connect(("127.0.0.1", PORT)).await.unwrap();
            let (r, mut w) = s.into_split();
            w.write_all(format!("{}\n", l.to_json(code)).as_bytes()).await.unwrap();
            let mut a = String::new();
            BufReader::new(r).read_line(&mut a).await.unwrap();
            a.trim().to_string()
        };
        assert_eq!(talk("1-0000-0000", l.clone()).await, "wrong");
        assert_eq!(talk("1-4829-1374", l.clone()).await, "ok");
        assert_eq!(tv.await.unwrap(), Ok(l.clone()));
        assert_eq!(stored(), Some(Login { ..l }));
        forget();
        let _ = std::fs::remove_dir_all(dir);
    }
}
