use std::env;
use std::process::exit;

fn main() {
    let url = env::var("NAVIDROME_URL").expect("set NAVIDROME_URL");
    let user = env::var("NAVIDROME_USER").expect("set NAVIDROME_USER");
    let password = env::var("NAVIDROME_PASSWORD").expect("set NAVIDROME_PASSWORD");

    let client = subsonic::Client::new(&url, &user, &password);

    match client.ping() {
        Ok(r) => println!(
            "Connected: {} {} (API {})\n",
            r.server_type.unwrap_or_default(),
            r.server_version.unwrap_or_default(),
            r.version
        ),
        Err(e) => {
            eprintln!("Ping failed: {e}");
            exit(1);
        }
    }

    match client.album_list(20, 0) {
        Ok(albums) => {
            for a in albums {
                let year = a.year.map(|y| format!(" ({y})")).unwrap_or_default();
                println!("{} — {}{}", a.artist.unwrap_or_default(), a.name, year);
            }
        }
        Err(e) => eprintln!("Album list failed: {e}"),
    }
}
