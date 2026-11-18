//! Course-level edits shared by every frontend: relocating a moved course,
//! setting its cover, and removing it from the library.

use crate::db::{queries, Connection};
use crate::error::{DeskemyError, Result};
use std::path::Path;
use std::sync::Mutex;

/// Where course covers are stored, under the data directory.
pub const THUMBNAILS_DIR: &str = "thumbnails";

/// Point a course at the folder it was moved or renamed to. Its lectures and
/// resources are re-pathed in place, so progress, bookmarks and tags survive.
/// Refuses a folder that doesn't hold the course's files (a sampled lecture
/// must exist at the same relative path). Returns the normalized new folder.
pub fn relocate(conn: &Connection, course_id: &str, new_folder: &str) -> Result<String> {
    let new_folder = new_folder.trim_end_matches(['/', '\\']).to_string();
    if !Path::new(&new_folder).is_dir() {
        return Err(DeskemyError::NotFound(format!("文件夹不存在：{new_folder}")));
    }
    let old_folder = queries::course_folder(conn, course_id)?
        .ok_or_else(|| DeskemyError::NotFound("course not found".into()))?;
    if let Some(sample) = queries::first_lecture_path(conn, course_id)? {
        if let Ok(rel) = Path::new(&sample).strip_prefix(&old_folder) {
            if !Path::new(&new_folder).join(rel).exists() {
                return Err(DeskemyError::Other(
                    "That folder doesn't contain this course's files. Pick the folder the \
                     course was moved or renamed to."
                        .into(),
                ));
            }
        }
    }
    queries::relocate_course(conn, course_id, &old_folder, &new_folder)?;
    Ok(new_folder)
}

/// Store image `bytes` as a course's cover (content-addressed under
/// `thumbs_dir`) and point the course at it. Returns the stored path.
pub fn set_cover(
    conn: &Connection,
    thumbs_dir: &Path,
    course_id: &str,
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<String> {
    let path = crate::thumbnails::store(thumbs_dir, bytes, ext_hint)?;
    let path = path.to_string_lossy().into_owned();
    queries::set_thumbnail(conn, course_id, Some(&path))?;
    Ok(path)
}

/// Remove a course from the library, in one transaction. Only the database
/// is touched — the files on disk stay where they are.
pub fn delete(conn: &mut Connection, course_id: &str) -> Result<()> {
    let tx = conn.transaction()?;
    queries::delete_course(&tx, course_id)?;
    tx.commit()?;
    Ok(())
}

/// Auto-generate a course thumbnail from its first video file.
/// Returns `true` if a new thumbnail was written, `false` if skipped.
/// Safe to call repeatedly — if a thumbnail already exists, does nothing.
pub fn ensure_thumbnail(conn: &Connection, course_id: &str, thumbs_dir: &Path) -> Result<bool> {
    // 已有缩略图就跳过
    let existing = queries::course_thumbnail(conn, course_id)?;
    if existing.is_some() {
        return Ok(false);
    }

    let Some(lecture_path) = queries::first_lecture_path(conn, course_id)? else {
        return Ok(false);
    };
    let video = Path::new(&lecture_path);
    if !video.exists() {
        return Ok(false);
    }

    match crate::thumbnails::grab_video_frame(video, thumbs_dir, 480)? {
        Some(path) => {
            let path_str = path.to_string_lossy().into_owned();
            queries::set_thumbnail(conn, course_id, Some(&path_str))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// One-off maintenance: a cover can point at the course folder's own image —
/// potentially a huge poster that decodes on the UI thread while scrolling.
/// Cache a downscaled copy (`thumbnails::COVER_MAX_WIDTH`) and repoint the
/// row, one course's lock at a time so the UI keeps using the database.
/// Returns how many courses were repointed.
pub fn cache_covers(db: &Mutex<Connection>, thumbs_dir: &Path) -> Result<usize> {
    let rows = {
        let conn = db.lock().unwrap_or_else(|e| e.into_inner());
        queries::course_covers(&conn)?
    };
    let mut fixed = 0;
    for (id, path) in rows {
        let src = Path::new(&path);
        if !src.exists() {
            continue;
        }
        let Some(stored) = crate::thumbnails::cache_cover(src, thumbs_dir, crate::thumbnails::COVER_MAX_WIDTH)?
        else {
            continue;
        };
        let conn = db.lock().unwrap_or_else(|e| e.into_inner());
        queries::set_thumbnail(&conn, &id, Some(&stored.to_string_lossy()))?;
        tracing::info!(course = %id, from = %path, to = %stored.display(), "cover cached");
        fixed += 1;
    }
    Ok(fixed)
}
