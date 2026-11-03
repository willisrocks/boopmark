//! The capture endpoint's body: one gzip stream of one document.

use flate2::bufread::GzDecoder;
use std::io::Read;

/// Why a captured payload was refused before any parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GzipError {
    /// Over the decompressed cap: a gzip bomb, or a page too large for
    /// either cap.
    TooLarge,
    /// Not gzip, truncated, or empty.
    Corrupt,
}

/// Inflate a gzip body, never producing more than `cap` bytes: the output is
/// bounded while inflating, so a bomb costs milliseconds, not memory. A
/// second gzip member or bytes after the compressed stream are `Corrupt`:
/// the extension sends one gzip stream of one document.
pub fn gunzip_capped(gz: &[u8], cap: usize) -> Result<Vec<u8>, GzipError> {
    let mut out = Vec::new();
    let mut decoder = GzDecoder::new(gz);
    // One byte more than the cap tells "exactly at the cap" from "over it".
    let limit = (cap as u64).saturating_add(1);
    decoder
        .by_ref()
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| GzipError::Corrupt)?;
    if out.len() > cap {
        return Err(GzipError::TooLarge);
    }
    if out.is_empty() {
        return Err(GzipError::Corrupt);
    }
    // `bufread::GzDecoder` stops at the end of its one member, so leftover
    // input is trailing garbage.
    if !decoder.get_ref().is_empty() {
        return Err(GzipError::Corrupt);
    }
    Ok(out)
}

/// Inflate off the async threads, and outside the parse slots: a corrupt or
/// oversized body is refused at once, even while every extraction slot is
/// busy with a hostile page.
pub async fn inflate_captured(
    gz: std::sync::Arc<Vec<u8>>,
    cap: usize,
) -> Result<Vec<u8>, GzipError> {
    tokio::task::spawn_blocking(move || gunzip_capped(&gz, cap))
        .await
        // A panic in the inflate is a bug, but the caller treats it like a
        // body it cannot read.
        .unwrap_or(Err(GzipError::Corrupt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn gzip_caps_are_exact_and_bombs_are_rejected() {
        let payload = vec![b'a'; 1000];
        let gz = gzip(&payload);
        assert_eq!(
            gunzip_capped(&gz, 1000).unwrap(),
            payload,
            "exactly at the cap"
        );
        assert_eq!(gunzip_capped(&gz, 999), Err(GzipError::TooLarge));
        assert_eq!(gunzip_capped(&gz, 5000).unwrap().len(), 1000);

        // A 100:1 bomb is refused at the output cap, not inflated whole.
        let bomb = gzip(&vec![0u8; 5 * 1024 * 1024]);
        assert_eq!(gunzip_capped(&bomb, 1024 * 1024), Err(GzipError::TooLarge));

        assert_eq!(gunzip_capped(b"not gzip", 100), Err(GzipError::Corrupt));
        assert_eq!(gunzip_capped(&[], 100), Err(GzipError::Corrupt));
        // Truncated inside the CRC trailer, with room for all the output.
        assert_eq!(
            gunzip_capped(&gz[..gz.len() - 3], 5000),
            Err(GzipError::Corrupt)
        );
        assert_eq!(gunzip_capped(&gzip(b""), 100), Err(GzipError::Corrupt));

        let mut trailing = gz.clone();
        trailing.extend_from_slice(b"garbage");
        assert_eq!(gunzip_capped(&trailing, 1000), Err(GzipError::Corrupt));
        // The extension sends one stream of one document: a second gzip
        // member is trailing input, and `Corrupt`.
        let mut two_members = gz.clone();
        two_members.extend_from_slice(&gzip(b"second"));
        assert_eq!(gunzip_capped(&two_members, 5000), Err(GzipError::Corrupt));
    }
}
