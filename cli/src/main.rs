use std::env;

fn main() {
    let url = env::var("NAVIDROME_URL").expect("set NAVIDROME_URL");
    let user = env::var("NAVIDROME_USER").expect("set NAVIDROME_USER");
    let password = env::var("NAVIDROME_PASSWORD").expect("set NAVIDROME_PASSWORD");

    let client = subsonic::Client::new(&url, &user, &password);

    match client.ping() {
        Ok(r) if r.status == "ok" => println!(
            "OK: {} {} (API {})",
            r.server_type.unwrap_or_default(),
            r.server_version.unwrap_or_default(),
            r.version
        ),
        Ok(r) => match r.error {
            Some(e) => eprintln!("Server error {}: {}", e.code, e.message),
            None => eprintln!("Unexpected status: {}", r.status),
        },
        Err(e) => eprintln!("Connection failed: {e}"),
    }
}
