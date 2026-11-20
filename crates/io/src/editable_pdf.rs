//! Editable PDF: a one-page PDF any reader shows (the flattened image), carrying the whole layered document as an
//! embedded file. Nephclick PhotoLab opens such a PDF back as that document, every layer editable; every other app sees
//! a normal PDF, with the document listed among its attachments.
//!
//! The PDF is written here directly (catalog, page, image XObject, embedded file), so its layout is known when it is
//! read back: [`extract`] finds the embedded document by the `/NephclickPhotoLab /Document` key of its stream's
//! dictionary.

use std::io::Write as _;

use nephclick_photolab_doc::Document;

use crate::IoError;

/// The key that marks the embedded document's stream.
const MARKER: &[u8] = b"/NephclickPhotoLab /Document";

/// Longest side of the page image, in pixels: larger documents are reduced for the page (the embedded document keeps
/// every pixel).
const MAX_PAGE_SIDE: u32 = 8000;

/// Largest embedded document read back (4 GiB).
const MAX_EMBEDDED: usize = 4 << 30;

/// The editable PDF of `doc`, whose layered document is `native` (the `.nplab` bytes).
pub fn write(doc: &Document, native: &[u8]) -> Result<Vec<u8>, IoError> {
    let side = doc.size.width.max(doc.size.height).clamp(1, MAX_PAGE_SIDE);
    let image = nephclick_photolab_compose::thumbnail(doc, side);
    let (w, h) = (image.width.max(1), image.height.max(1));
    // The page: the composite over white, RGB, deflated.
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in image.pixels.chunks_exact(4) {
        let a = u32::from(px[3]);
        for c in 0..3 {
            rgb.push(((u32::from(px[c]) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&rgb).map_err(|e| IoError::Unsupported(e.to_string()))?;
    let pixels = z.finish().map_err(|e| IoError::Unsupported(e.to_string()))?;
    drop(rgb);
    // The page in points, from the document's resolution.
    let dpi = if doc.resolution_dpi.is_finite() && doc.resolution_dpi > 0.0 { f64::from(doc.resolution_dpi) } else { 72.0 };
    let pw = (f64::from(doc.size.width.max(1)) * 72.0 / dpi).max(1.0);
    let ph = (f64::from(doc.size.height.max(1)) * 72.0 / dpi).max(1.0);
    let file_name = pdf_string(&format!("{}.{}", stem(&doc.name), nephclick_photolab_format::EXTENSION));

    let mut out: Vec<u8> = Vec::with_capacity(pixels.len() + native.len() + 4096);
    let mut offsets = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, head: String, stream: Option<&[u8]>| {
        offsets.push(out.len());
        out.extend_from_slice(head.as_bytes());
        if let Some(data) = stream {
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offsets, format!("1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{file_name} 7 0 R] >> >> /AF [7 0 R] >>"), None);
    obj(&mut out, &mut offsets, "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(), None);
    obj(
        &mut out,
        &mut offsets,
        format!("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"),
        None,
    );
    obj(
        &mut out,
        &mut offsets,
        format!("4 0 obj\n<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>", pixels.len()),
        Some(&pixels),
    );
    let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q");
    obj(&mut out, &mut offsets, format!("5 0 obj\n<< /Length {} >>", content.len()), Some(content.as_bytes()));
    obj(
        &mut out,
        &mut offsets,
        format!(
            "6 0 obj\n<< /Type /EmbeddedFile /Subtype /application#2Fx-nephclick-photolab {} /Params << /Size {} >> /Length {} >>",
            String::from_utf8_lossy(MARKER),
            native.len(),
            native.len()
        ),
        Some(native),
    );
    obj(
        &mut out,
        &mut offsets,
        format!("7 0 obj\n<< /Type /Filespec /F {file_name} /UF {file_name} /EF << /F 6 0 R >> /Desc (Nephclick PhotoLab document: open this PDF in Nephclick PhotoLab to edit its layers) /AFRelationship /Source >>"),
        None,
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    Ok(out)
}

/// The layered document an editable PDF carries (the `.nplab` bytes); `None` for any other PDF.
pub fn extract(pdf: &[u8]) -> Option<&[u8]> {
    if !pdf.starts_with(b"%PDF-") {
        return None;
    }
    let at = find(pdf, MARKER, 0)?;
    // The stream dictionary ends at `>>` before `stream`; its `/Length` is the document's size.
    let dict_end = find(pdf, b"stream", at)?;
    let dict = pdf.get(at..dict_end)?;
    let len_at = find(dict, b"/Length ", 0)? + b"/Length ".len();
    let digits: Vec<u8> = dict.get(len_at..)?.iter().copied().take_while(u8::is_ascii_digit).collect();
    let len: usize = std::str::from_utf8(&digits).ok()?.parse().ok()?;
    if len == 0 || len > MAX_EMBEDDED {
        return None;
    }
    let mut start = dict_end + b"stream".len();
    if pdf.get(start) == Some(&b'\r') {
        start += 1;
    }
    if pdf.get(start) == Some(&b'\n') {
        start += 1;
    }
    pdf.get(start..start.checked_add(len)?)
}

/// The first `needle` in `hay` at or after `from`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// `name` without a final extension.
fn stem(name: &str) -> &str {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ if name.is_empty() => "document",
        _ => name,
    }
}

/// `text` as a PDF literal string: ASCII kept, `(`, `)` and `\` escaped, anything else replaced by `_`.
fn pdf_string(text: &str) -> String {
    let mut s = String::from("(");
    for c in text.chars().take(120) {
        match c {
            '(' | ')' | '\\' => {
                s.push('\\');
                s.push(c);
            }
            c if c.is_ascii() && !c.is_ascii_control() => s.push(c),
            _ => s.push('_'),
        }
    }
    s.push(')');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_embedded_document_comes_back_and_other_pdfs_carry_none() {
        let doc = Document::new("Poster (final).psd", nephclick_photolab_geom::Size { width: 8, height: 4 }, nephclick_photolab_color::ColorMode::Rgb, nephclick_photolab_color::SampleType::U8);
        let pdf = write(&doc, b"PK-the-document").unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert_eq!(extract(&pdf), Some(&b"PK-the-document"[..]));
        assert_eq!(extract(b"%PDF-1.4\n1 0 obj << >> endobj"), None);
        assert_eq!(extract(b"not a pdf"), None);
        // A damaged length never reads past the end.
        let mut broken = pdf.clone();
        broken.truncate(broken.len() / 2);
        let _ = extract(&broken);
    }
}
