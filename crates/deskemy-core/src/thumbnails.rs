//! Course thumbnail storage. Images (user-picked or pasted) are written into
//! the app data dir, content-addressed by hash, so identical images dedupe and
//! the asset protocol can serve them by a stable path.
//!
//! Also grabs video frames headlessly via libmpv and encodes them as JPG.

use crate::error::{DeskemyError, Result};
use crate::mpv::{MPV_EVENT_END_FILE, MPV_EVENT_FILE_LOADED};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Best-effort image type detection from magic bytes.
pub fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Normalize an extension/MIME hint (e.g. ".JPEG" or "image/png") to a bare,
/// known image extension, defaulting to png.
fn sanitize_ext(hint: &str) -> &'static str {
    let lower = hint.trim_start_matches('.').to_lowercase();
    match lower.rsplit('/').next().unwrap_or(&lower) {
        "jpeg" | "jpg" => "jpg",
        "png" => "png",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        _ => "png",
    }
}

/// Write `bytes` into `thumbs_dir`, named by content hash. Returns the stored
/// absolute path. The extension is taken from the bytes' magic number when
/// recognizable, else from `ext_hint`, else png.
pub fn store(thumbs_dir: &Path, bytes: &[u8], ext_hint: Option<&str>) -> Result<PathBuf> {
    if bytes.is_empty() {
        return Err(DeskemyError::Other("图片数据为空".into()));
    }
    let ext = sniff_ext(bytes).unwrap_or_else(|| ext_hint.map(sanitize_ext).unwrap_or("png"));
    std::fs::create_dir_all(thumbs_dir)?;
    let hash = blake3::hash(bytes).to_hex();
    let path = thumbs_dir.join(format!("{}.{ext}", &hash[..16]));
    if !path.exists() {
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

/// Covers are shown at card / hero size (~280-440 device px); anything wider
/// is wasted decode and upload. An 11300x6500 JPEG — a course folder's own
/// poster — decodes into a 294 MB pixel buffer on the UI thread and stalls
/// scrolling.
pub const COVER_MAX_WIDTH: u32 = 800;

/// An image file's pixel dimensions, from its header: PNG's IHDR or a JPEG
/// start-of-frame marker. Covers come in these two; `None` for anything else.
pub fn image_size(path: &Path) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        if data.len() < 24 {
            return None;
        }
        let (w, h) = (
            u32::from_be_bytes([data[16], data[17], data[18], data[19]]),
            u32::from_be_bytes([data[20], data[21], data[22], data[23]]),
        );
        return Some((w, h));
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // Walk the segment chain until a start-of-frame (SOFn) marker.
        let mut i = 2;
        while i + 9 <= data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
            if is_sof {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            if marker == 0x01 || marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
                i += 2;
                continue;
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + len;
        }
    }
    None
}

/// Downscale an image file into the content-addressed cache, at most
/// `max_width` px wide, as JPG. Returns the stored path; `None` when there is
/// nothing to do (the image is at most `max_width` wide, or its size can't be
/// read) or ffmpeg is missing / the encode fails — the caller keeps the
/// original then.
pub fn cache_cover(src: &Path, thumbs_dir: &Path, max_width: u32) -> Result<Option<PathBuf>> {
    let Some((w, _h)) = image_size(src) else {
        return Ok(None);
    };
    if w <= max_width {
        return Ok(None);
    }
    let Some(ffmpeg) = find_ffmpeg() else {
        return Ok(None);
    };
    let tmp = std::env::temp_dir().join(format!(
        "deskemy_cover_{}_{:x}.jpg",
        blake3::hash(src.to_string_lossy().as_bytes()).to_hex()[..8].to_string(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let output = std::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(src)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(format!("scale={max_width}:-2"))
        .arg("-q:v")
        .arg("5")
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()?;
    if !output.status.success() || !tmp.exists() {
        tracing::warn!(
            path = %src.display(),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "cover: ffmpeg failed"
        );
        return Ok(None);
    }
    let bytes = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok(Some(store(thumbs_dir, &bytes, Some("jpg"))?))
}

/// Grab a frame from a video file headlessly via libmpv software rendering,
/// encode it as JPG, and store it in `thumbs_dir`. Returns the stored path.
///
/// Picks a frame at 40% of the video duration (past any intro, before credits).
/// Downscales to at most `max_width` pixels wide to keep files small.
/// Returns `Ok(None)` if libmpv is unavailable or the video can't be opened.
pub fn grab_video_frame(
    video_path: &Path,
    thumbs_dir: &Path,
    max_width: u32,
) -> Result<Option<PathBuf>> {
    let ffmpeg = match find_ffmpeg() {
        Some(p) => p,
        None => return Ok(None),
    };

    let tmp_jpg = std::env::temp_dir().join(format!(
        "deskemy_thumb_{}.jpg",
        blake3::hash(video_path.to_string_lossy().as_bytes()).to_hex()[..8].to_string()
    ));
    if tmp_jpg.exists() {
        let _ = std::fs::remove_file(&tmp_jpg);
    }

    // 先拿视频时长（mpv 能快速返回），算 40% 位置秒数
    let seek_secs = match probe_duration(video_path) {
        Some(d) if d > 1.0 => (d * 0.4).to_string(),
        _ => "5".to_string(), // 兜底：跳过前 5 秒
    };

    // ffmpeg: seek 到 40% 位置，抽一帧，缩放到 max_width，输出 JPG
    let child = std::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(&seek_secs)
        .arg("-i")
        .arg(video_path)
        .arg("-vframes")
        .arg("1")
        .arg("-vf")
        .arg(format!("scale={}:-2", max_width))
        .arg("-q:v")
        .arg("5")
        .arg(&tmp_jpg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let output = child.wait_with_output()?;
    if !output.status.success() {
        tracing::warn!(
            path = %video_path.display(),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "thumbnail: ffmpeg failed"
        );
        return Ok(None);
    }

    if !tmp_jpg.exists() {
        return Ok(None);
    }

    let raw_bytes = std::fs::read(&tmp_jpg)?;
    let _ = std::fs::remove_file(&tmp_jpg);

    let stored = store(thumbs_dir, &raw_bytes, Some("jpg"))?;
    Ok(Some(stored))
}

/// Find bundled ffmpeg.exe in vendor dir (next to libmpv-2.dll/pdfium.dll).
fn find_ffmpeg() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates = [
        exe_dir.join("ffmpeg.exe"),
        exe_dir.join("vendor").join("ffmpeg.exe"),
        exe_dir.parent()?.parent()?.parent()?.join("vendor").join("ffmpeg.exe"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Quick duration probe via libmpv (headless, vo=null). Returns seconds.
fn probe_duration(video_path: &Path) -> Option<f64> {
    use crate::mpv::Mpv;
    if !crate::mpv::is_available() {
        return None;
    }
    let mpv = Mpv::new().ok()?;
    let _ = mpv.set_option("vo", "null");
    let _ = mpv.set_option("ao", "null");
    let _ = mpv.set_option("pause", "yes");
    let _ = mpv.set_option("idle", "yes");
    let _ = mpv.set_option("terminal", "no");
    let _ = mpv.initialize();

    let _ = mpv.command(&[
        "loadfile",
        &crate::mpv::file_arg(&video_path.to_string_lossy()),
    ]);
    // 等 demux 出来
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let ev = mpv.wait_event(0.2);
        if !ev.is_null() {
            match unsafe { (*ev).event_id } {
                MPV_EVENT_FILE_LOADED | MPV_EVENT_END_FILE => break,
                _ => {}
            }
        }
    }
    mpv.get_f64("duration").filter(|&d| d > 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A still frame of the given size, via the bundled ffmpeg. None without
    /// it (the tests skip then).
    fn gen_image(dir: &Path, name: &str, size: &str) -> Option<PathBuf> {
        let ffmpeg = find_ffmpeg()?;
        let out = dir.join(name);
        let status = std::process::Command::new(ffmpeg)
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size={size}:duration=0.1"))
            .args(["-frames:v", "1"])
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        assert!(status.success(), "ffmpeg testsrc failed");
        Some(out)
    }

    #[test]
    fn image_size_reads_png_and_jpeg_headers() {
        let Some(ffmpeg) = find_ffmpeg() else {
            eprintln!("ffmpeg unavailable; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for (name, size, expected) in [
            ("a.png", "640x360", (640, 360)),
            ("b.jpg", "640x360", (640, 360)),
        ] {
            let path = dir.path().join(name);
            let status = std::process::Command::new(&ffmpeg)
                .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(format!("testsrc=size={size}:duration=0.1"))
                .args(["-frames:v", "1"])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
            assert_eq!(image_size(&path), Some(expected), "{name}");
        }
        let junk = dir.path().join("junk.bin");
        std::fs::write(&junk, b"not an image").unwrap();
        assert_eq!(image_size(&junk), None);
    }

    #[test]
    fn cache_cover_downscales_wide_and_skips_small() {
        if find_ffmpeg().is_none() {
            eprintln!("ffmpeg unavailable; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let thumbs = dir.path().join("thumbs");
        let big = gen_image(dir.path(), "big.jpg", "1200x700").unwrap();
        assert_eq!(image_size(&big), Some((1200, 700)));
        let stored = cache_cover(&big, &thumbs, COVER_MAX_WIDTH).unwrap().expect("1200px wide is cached");
        assert_eq!(image_size(&stored).unwrap().0, COVER_MAX_WIDTH);
        assert!(stored.starts_with(&thumbs));

        // The small one needs no work; the oversized one is now a no-op too.
        let small = gen_image(dir.path(), "small.jpg", "400x225").unwrap();
        assert_eq!(cache_cover(&small, &thumbs, COVER_MAX_WIDTH).unwrap(), None);
        assert_eq!(cache_cover(&stored, &thumbs, COVER_MAX_WIDTH).unwrap(), None);
    }
}
