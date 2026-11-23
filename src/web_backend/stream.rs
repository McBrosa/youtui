use serde_json::{Value, json};

/// Select a browser-playable source from yt-dlp's formats, ordered worst to best.
pub(super) fn browser_source(info: &Value, native_hls: bool) -> Option<Value> {
    if info["is_live"] == true {
        return None;
    }
    let formats = info["formats"].as_array()?;
    let videos = formats.iter().rev().filter(|format| {
        format["ext"] == "mp4"
            && format["has_drm"] != true
            && format["vcodec"]
                .as_str()
                .is_some_and(|codec| codec.starts_with("avc1"))
            && format["height"]
                .as_u64()
                .is_some_and(|height| (1..=720).contains(&height))
    });
    for format in videos.clone() {
        if format["protocol"] == "https"
            && format["acodec"]
                .as_str()
                .is_some_and(|codec| codec.starts_with("mp4a"))
            && let Some(url) = cdn_url(format)
        {
            return Some(json!({"mime": "video/mp4", "stream_url": url}));
        }
    }
    if !native_hls {
        return None;
    }
    // A video-only variant playlist needs an audio rendition from the same master.
    for video in videos.filter(|format| hls_playlist(format) && format["acodec"] == "none") {
        let Some(video_url) = cdn_url(video) else {
            continue;
        };
        let Some(master) = video["manifest_url"].as_str() else {
            continue;
        };
        let audio_rendition = formats
            .iter()
            .rev()
            .filter(|audio| {
                audio["manifest_url"] == master
                    && hls_playlist(audio)
                    && audio["vcodec"] == "none"
                    && audio["ext"] == "mp4"
                    && audio["has_drm"] != true
                    && audio["acodec"]
                        .as_str()
                        .is_none_or(|codec| codec.starts_with("mp4a"))
            })
            .find_map(|audio| Some((cdn_url(audio)?, audio)));
        if let Some((audio_url, audio)) = audio_rendition {
            let kbps =
                video["tbr"].as_f64().unwrap_or(3_000.0) + audio["tbr"].as_f64().unwrap_or(128.0);
            return Some(json!({
                "mime": "application/vnd.apple.mpegurl",
                "video_playlist_url": video_url,
                "audio_playlist_url": audio_url,
                "video_codec": video["vcodec"],
                "audio_codec": audio["acodec"],
                "width": video["width"],
                "height": video["height"],
                "bandwidth": (kbps * 1000.0).clamp(1.0, 100_000_000.0) as u64
            }));
        }
    }
    None
}

fn hls_playlist(format: &Value) -> bool {
    matches!(format["protocol"].as_str(), Some("m3u8" | "m3u8_native"))
}

fn cdn_url(format: &Value) -> Option<&str> {
    let url = format["url"].as_str()?;
    let (host, _) = url.strip_prefix("https://")?.split_once('/')?;
    let valid_host = host.ends_with(".googlevideo.com")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    let valid_url = url.len() <= 16 * 1024
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'\\' | b'#' | b'"'));
    (valid_host && valid_url).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_pairs_compatible_renditions_from_one_master_when_the_browser_supports_it() {
        let video = json!({
            "protocol":"m3u8_native", "ext":"mp4", "height":720, "tbr":1234.0,
            "vcodec":"avc1.4d401f", "acodec":"none", "manifest_url":"master",
            "url":"https://manifest.googlevideo.com/video"
        });
        let audio = json!({
            "protocol":"m3u8_native", "ext":"mp4", "vcodec":"none", "tbr":96.0, "manifest_url":"master",
            "url":"https://manifest.googlevideo.com/audio"
        });
        let mut info = json!({"formats":[audio, video]});
        // The complete recording exceeds the file cache limit but can stream in segments.
        info["formats"][1]["filesize_approx"] = json!(2_000_000_000);
        assert!(browser_source(&info, false).is_none());
        let source = browser_source(&info, true).unwrap();
        assert_eq!(source["video_playlist_url"], video["url"]);
        assert_eq!(source["audio_playlist_url"], audio["url"]);
        assert_eq!(source["mime"], "application/vnd.apple.mpegurl");
        assert_eq!(source["bandwidth"], 1_330_000);

        info["formats"][0]["manifest_url"] = json!("different master");
        assert!(browser_source(&info, true).is_none());
        info["formats"][0]["manifest_url"] = json!("master");
        info["formats"][1]["height"] = json!(1080);
        assert!(browser_source(&info, true).is_none());
    }
}
