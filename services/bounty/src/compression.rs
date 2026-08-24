use std::process::Command;
use std::fs;
use tempfile::tempdir;

pub async fn compress_video(video_data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let dir = tempdir()?;
    let input_path = dir.path().join("input.mp4");
    let output_path = dir.path().join("output.mp4");

    fs::write(&input_path, video_data)?;

    let status = Command::new("ffmpeg")
        .arg("-i")
        .arg(&input_path)
        .arg("-vcodec")
        .arg("libx264")
        .arg("-crf")
        .arg("28")
        .arg(&output_path)
        .status()?;

    if !status.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "ffmpeg failed"));
    }

    let compressed_data = fs::read(&output_path)?;
    Ok(compressed_data)
}
