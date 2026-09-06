use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::media::ytdlp::AudioQuality;

/// Convert any audio source to MP3 at the requested bitrate.
///
/// Runs `ffmpeg -i <src> -vn -c:a libmp3lame -b:a <bitrate> <dst>`.
pub async fn to_mp3(
    src: &Path,
    dst: &Path,
    quality: AudioQuality,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &src.to_string_lossy(),
            "-vn",
            "-c:a",
            "libmp3lame",
            "-b:a",
            quality.mp3_bitrate(),
            &dst.to_string_lossy(),
        ])
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Convert(format!("failed to start ffmpeg: {e}")))?;

    let status = tokio::select! {
        status = child.wait() => status.map_err(|e| Error::Convert(e.to_string()))?,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(Error::Cancelled);
        }
    };

    if !status.success() {
        return Err(Error::Convert("ffmpeg exited with an error".to_owned()));
    }
    if !tokio::fs::try_exists(dst).await.unwrap_or(false) {
        return Err(Error::Convert("ffmpeg produced no file".to_owned()));
    }
    Ok(())
}

/// Build ffmpeg args (pure, for tests/docs).
#[cfg(test)]
#[must_use]
pub fn mp3_args(src: &Path, dst: &Path, quality: AudioQuality) -> Vec<String> {
    vec![
        "-y".to_owned(),
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-i".to_owned(),
        src.to_string_lossy().into_owned(),
        "-vn".to_owned(),
        "-c:a".to_owned(),
        "libmp3lame".to_owned(),
        "-b:a".to_owned(),
        quality.mp3_bitrate().to_owned(),
        dst.to_string_lossy().into_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_contain_bitrate() {
        let args = mp3_args(
            Path::new("in.m4a"),
            Path::new("out.mp3"),
            AudioQuality::K128,
        );
        assert!(args.contains(&"128k".to_owned()));
        assert!(args.contains(&"libmp3lame".to_owned()));
    }
}
