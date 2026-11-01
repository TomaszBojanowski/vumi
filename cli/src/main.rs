use gst::prelude::*;
use gstreamer as gst;
use std::env;
use std::process::exit;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> AnyResult<()> {
    let url = env::var("NAVIDROME_URL").expect("set NAVIDROME_URL");
    let user = env::var("NAVIDROME_USER").expect("set NAVIDROME_USER");
    let password = env::var("NAVIDROME_PASSWORD").expect("set NAVIDROME_PASSWORD");

    let client = subsonic::Client::new(&url, &user, &password);

    match client.ping().await {
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

    let albums = client.album_list(20, 0).await?;
    for (i, a) in albums.iter().enumerate() {
        let year = a.year.map(|y| format!(" ({y})")).unwrap_or_default();
        println!(
            "{:>2}. {} — {}{}",
            i + 1,
            a.artist.as_deref().unwrap_or(""),
            a.name,
            year
        );
    }

    let Some(arg) = env::args().nth(1) else {
        println!("\nRun with an album number to play it, e.g.: cargo run -p cli -- 3");
        return Ok(());
    };
    let n: usize = arg
        .parse()
        .map_err(|_| "album number must be a positive integer")?;
    if n == 0 || n > albums.len() {
        return Err(format!("pick a number from 1 to {}", albums.len()).into());
    }

    let album = client.album(&albums[n - 1].id).await?;
    println!(
        "\nPlaying: {} — {}\n",
        album.artist.as_deref().unwrap_or(""),
        album.name
    );

    gst::init()?;
    for song in &album.song {
        println!(
            "▶ {:>2}. {} [{} {} kbps]",
            song.track.unwrap_or(0),
            song.title,
            song.suffix.as_deref().unwrap_or("?"),
            song.bit_rate.unwrap_or(0)
        );
        let stream = client.stream_url(&song.id)?;
        // Blocking playback is fine in this test tool: nothing else runs meanwhile.
        play(stream.as_str())?;
    }
    Ok(())
}

/// Plays one URI with playbin and blocks until it ends or fails.
fn play(uri: &str) -> AnyResult<()> {
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri)
        .build()?;
    playbin.set_state(gst::State::Playing)?;

    let bus = playbin.bus().ok_or("playbin has no bus")?;
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        match msg.view() {
            gst::MessageView::Eos(..) => break,
            gst::MessageView::Error(err) => {
                playbin.set_state(gst::State::Null)?;
                return Err(format!("{} ({:?})", err.error(), err.debug()).into());
            }
            _ => (),
        }
    }
    playbin.set_state(gst::State::Null)?;
    Ok(())
}
