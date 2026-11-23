use std::path::Path;

use serde_json::Value;

use super::{ResolvedStream, ffmpeg_headers};
use crate::config::PixelVideoQuality;
use crate::ipc::IpcClient;

/// A separate IPC connection keeps video lookup off mpv's playback-control channel.
pub(super) fn from_player(
    socket: &Path,
    video_id: &str,
    quality: PixelVideoQuality,
) -> Option<ResolvedStream> {
    let mut ipc = IpcClient::connect(socket).ok()?;
    let data = ipc
        .send_command_with_data(&[
            "get_property",
            "user-data/mpv/ytdl/json-subprocess-result/stdout",
        ])
        .ok()??;
    let info = serde_json::from_str(data.as_str()?).ok()?;
    from_metadata(&info, video_id, quality)
}

fn from_metadata(
    info: &Value,
    video_id: &str,
    quality: PixelVideoQuality,
) -> Option<ResolvedStream> {
    if info["id"].as_str()? != video_id || info["is_live"] == true {
        return None;
    }
    // yt-dlp lists formats from worst to best. Prefer capped H.264 video-only streams,
    // then other direct video files. Fragment lists and manifests use the normal resolver.
    let format = info["formats"]
        .as_array()?
        .iter()
        .filter_map(|format| {
            let codec = format["vcodec"].as_str()?;
            let height = format["height"].as_u64()?;
            let url = format["url"].as_str()?;
            if !matches!(format["protocol"].as_str(), Some("http" | "https"))
                || !(url.starts_with("https://") || url.starts_with("http://"))
                || url.bytes().any(|byte| byte.is_ascii_control())
                || format["has_drm"] == true
                || format["fragments"]
                    .as_array()
                    .is_some_and(|fragments| !fragments.is_empty())
                || codec == "none"
                || !(1..=u64::from(quality.height())).contains(&height)
            {
                return None;
            }
            let rank = (
                format["acodec"] == "none",
                codec.starts_with("avc1"),
                height,
            );
            Some((rank, format))
        })
        .max_by_key(|(rank, _)| *rank)?
        .1;

    Some(ResolvedStream {
        url: format["url"].as_str()?.to_owned(),
        http_headers: ffmpeg_headers(
            format
                .get("http_headers")
                .or_else(|| info.get("http_headers")),
        ),
        quality,
        from_audio_player: true,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;
    use serde_json::json;

    #[test]
    fn reuses_only_a_matching_tracks_compatible_video_and_headers() {
        let mut format = json!({"url":"https://test/video", "protocol":"https", "vcodec":"avc1",
            "acodec":"none", "height":240, "http_headers":{"User-Agent":"player", "X-Bad":"a\nb"}});
        let mut formats = vec![format.clone()];
        for (key, value) in [
            ("height", json!(720)),
            ("has_drm", json!(true)),
            ("vcodec", json!("none")),
            ("protocol", json!("http_dash_segments")),
            ("fragments", json!([{"url": "https://test/fragment"}])),
            ("url", json!("file:///test")),
        ] {
            let mut unsupported = format.clone();
            unsupported["url"] = json!("https://test/unsupported");
            unsupported[key] = value;
            formats.push(unsupported);
        }
        format["vcodec"] = json!("av01");
        format["url"] = json!("https://test/other");
        formats.push(format);
        let info = json!({"id":"abc","formats":formats});
        let stream = from_metadata(&info, "abc", PixelVideoQuality::P240).unwrap();
        assert_eq!(stream.url, "https://test/video");
        assert_eq!(stream.http_headers, "User-Agent: player\r\n");
        assert!(from_metadata(&info, "previous-track", PixelVideoQuality::P240).is_none());
        assert!(from_metadata(&info, "abc", PixelVideoQuality::P144).is_none());
        let mut live = info;
        live["is_live"] = json!(true);
        assert!(from_metadata(&live, "abc", PixelVideoQuality::P240).is_none());
    }

    #[test]
    fn reads_extractor_stdout_over_mpv_ipc_only_for_the_requested_track() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            for id in ["playing", "previous"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(
                    request["command"],
                    json!([
                        "get_property",
                        "user-data/mpv/ytdl/json-subprocess-result/stdout"
                    ])
                );
                let stdout = json!({"id": id, "formats": [{
                    "url": "https://video.test/stream", "protocol": "https", "height": 240,
                    "vcodec": "avc1", "acodec": "none", "http_headers": {"User-Agent": "test"}
                }]});
                let reply = json!({
                    "request_id": request["request_id"], "error": "success", "data": stdout.to_string()
                });
                writeln!(stream, "{reply}").unwrap();
            }
        });

        let stream = from_player(&socket, "playing", PixelVideoQuality::P240).unwrap();
        assert_eq!(stream.url, "https://video.test/stream");
        assert_eq!(stream.http_headers, "User-Agent: test\r\n");
        assert!(stream.from_audio_player);
        assert!(from_player(&socket, "playing", PixelVideoQuality::P240).is_none());
        server.join().unwrap();
    }
}
