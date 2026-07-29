use qrcode::{Color, QrCode};

pub fn print_url_qr(url: &str) -> Result<(), String> {
    let code = QrCode::new(url.as_bytes())
        .map_err(|error| format!("接続 URL の QR コードを生成できません: {error}"))?;
    let quiet_zone = 2;
    let width = code.width();
    println!("接続 URL: {url}");
    for y in 0..(width + quiet_zone * 2) {
        let mut line = String::new();
        for x in 0..(width + quiet_zone * 2) {
            let dark = x >= quiet_zone
                && y >= quiet_zone
                && x < width + quiet_zone
                && y < width + quiet_zone
                && code[(x - quiet_zone, y - quiet_zone)] == Color::Dark;
            line.push_str(if dark {
                "\x1b[30;40m██"
            } else {
                "\x1b[97;47m██"
            });
        }
        line.push_str("\x1b[0m");
        println!("{line}");
    }
    Ok(())
}
