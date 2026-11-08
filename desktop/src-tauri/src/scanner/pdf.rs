//! Turning scanned pages into one PDF.
//!
//! A scanner hands back pictures — JPEG from an eSCL scanner or SANE in
//! colour and grey, PNG from SANE in black and white — and what the user wants
//! in their drive is a document. This writes the PDF by hand rather than
//! pulling in a PDF library, because the job is narrow enough to do exactly:
//! each page is one image, drawn to fill a page the size the scanner says it
//! is, and **neither format is decoded or re-encoded**.
//!
//! - JPEG goes in as it is, under `/DCTDecode`. PDF readers decode JPEG
//!   natively, so the bytes the scanner produced are the bytes in the file.
//! - PNG's image data is a zlib stream of filtered scanlines, and PDF's
//!   `/FlateDecode` with `/Predictor 15` is defined to read exactly that. So
//!   the `IDAT` chunks are concatenated and embedded untouched, with no zlib
//!   dependency and no loss.
//!
//! Re-encoding would be both slower and worse: a second JPEG pass costs
//! quality, and decoding a 600 dpi colour page to raw pixels is ~100 MB.

use std::fmt::Write as _;

/// How a page's image data is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
}

/// One scanned page, as the scanner returned it.
#[derive(Debug, Clone)]
pub struct Page {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
    /// Dots per inch the page was scanned at. It is what turns pixels into a
    /// page size: a 2480-pixel-wide scan at 300 dpi is 8.27 inches, A4.
    pub dpi: u32,
}

/// What a page's image header says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    /// 1 grey, 3 RGB, 4 CMYK.
    pub components: u8,
    pub bits: u8,
    /// Set for an Adobe CMYK JPEG, whose values are stored inverted.
    pub adobe_inverted: bool,
}

/// Document-level properties written into the PDF's Info dictionary.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub title: String,
    /// Written as `/Keywords`. The drive's own tag index is what MeCloud
    /// searches, but a PDF that leaves the drive — downloaded, mailed,
    /// synced elsewhere — keeps its tags this way, and desktop search tools
    /// index the field.
    pub keywords: Vec<String>,
    /// `D:YYYYMMDDHHmmSSZ`, or empty to leave the date out.
    pub created: String,
}

/// Read width, height and colour layout from a JPEG's start-of-frame marker.
pub fn jpeg_info(data: &[u8]) -> Result<ImageInfo, String> {
    const BAD: &str = "The scanner returned an image that is not a readable JPEG.";
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(BAD.into());
    }
    let mut adobe_inverted = false;
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return Err(BAD.into());
        }
        let marker = data[i + 1];
        // Fill bytes, and the markers that carry no length.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return Err(BAD.into());
        }
        let segment = &data[i + 4..i + 2 + len];
        // APP14 "Adobe": a CMYK JPEG written by Adobe's convention stores the
        // channels inverted, and a PDF has to be told to flip them back.
        if marker == 0xEE && segment.starts_with(b"Adobe") {
            adobe_inverted = true;
        }
        // SOF0..SOF15, except DHT (C4), JPG (C8) and DAC (CC).
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            if segment.len() < 6 {
                return Err(BAD.into());
            }
            let bits = segment[0];
            let height = u16::from_be_bytes([segment[1], segment[2]]) as u32;
            let width = u16::from_be_bytes([segment[3], segment[4]]) as u32;
            let components = segment[5];
            if width == 0 || height == 0 || !matches!(components, 1 | 3 | 4) {
                return Err(BAD.into());
            }
            return Ok(ImageInfo { width, height, components, bits, adobe_inverted });
        }
        if marker == 0xDA {
            // Start of scan with no frame header before it.
            break;
        }
        i += 2 + len;
    }
    Err(BAD.into())
}

/// A PNG's header, and its image data with the chunk framing removed.
pub fn png_parts(data: &[u8]) -> Result<(ImageInfo, Vec<u8>), String> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    const BAD: &str = "The scanner returned an image that is not a readable PNG.";
    if !data.starts_with(SIGNATURE) {
        return Err(BAD.into());
    }
    let mut i = SIGNATURE.len();
    let mut info: Option<ImageInfo> = None;
    let mut idat = Vec::with_capacity(data.len());
    while i + 8 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let kind = &data[i + 4..i + 8];
        let start = i + 8;
        let end = start.checked_add(len).filter(|&e| e + 4 <= data.len()).ok_or(BAD)?;
        let body = &data[start..end];
        match kind {
            b"IHDR" => {
                if body.len() < 13 {
                    return Err(BAD.into());
                }
                let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                let bits = body[8];
                let colour = body[9];
                let interlace = body[12];
                // Grey or RGB, no alpha, no palette, not interlaced: the
                // shapes a scanner produces, and the ones a PDF predictor
                // reads as they are. Anything else would need decoding.
                let components = match colour {
                    0 => 1,
                    2 => 3,
                    _ => return Err("The scanner returned a PNG in a colour layout this client cannot embed.".into()),
                };
                if interlace != 0 {
                    return Err("The scanner returned an interlaced PNG, which this client cannot embed.".into());
                }
                if !matches!(bits, 1 | 2 | 4 | 8 | 16) || width == 0 || height == 0 {
                    return Err(BAD.into());
                }
                info = Some(ImageInfo { width, height, components, bits, adobe_inverted: false });
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i = end + 4; // skip the CRC
    }
    let info = info.ok_or(BAD)?;
    if idat.is_empty() {
        return Err(BAD.into());
    }
    Ok((info, idat))
}

/// Escape a string for a PDF literal string.
///
/// Text outside ASCII is written as UTF-16BE with a byte-order mark, the form
/// PDF defines for text strings, as a hex string so no byte needs escaping.
fn pdf_text(value: &str) -> String {
    if value.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in value.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for unit in value.encode_utf16() {
            let _ = write!(out, "{unit:04X}");
        }
        out.push('>');
        out
    }
}

/// Points (1/72 inch) for `pixels` at `dpi`.
fn points(pixels: u32, dpi: u32) -> f64 {
    let dpi = if dpi == 0 { 300 } else { dpi };
    pixels as f64 * 72.0 / dpi as f64
}

/// Build the PDF.
pub fn build(pages: &[Page], meta: &Meta) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("Nothing was scanned.".into());
    }

    // Objects are numbered: 1 catalog, 2 page tree, 3 info, then three per
    // page (page, content stream, image).
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let page_ids: Vec<usize> = (0..pages.len()).map(|i| 4 + i * 3).collect();
    let mut objects: Vec<(usize, Vec<u8>)> = Vec::new();

    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
    objects.push((2, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).into_bytes()));

    let mut info = String::from("<< /Producer ");
    info.push_str(&pdf_text(concat!("MeCloud Desktop ", env!("CARGO_PKG_VERSION"))));
    if !meta.title.is_empty() {
        info.push_str(" /Title ");
        info.push_str(&pdf_text(&meta.title));
    }
    if !meta.keywords.is_empty() {
        info.push_str(" /Keywords ");
        info.push_str(&pdf_text(&meta.keywords.join(", ")));
    }
    if !meta.created.is_empty() {
        info.push_str(" /CreationDate ");
        info.push_str(&pdf_text(&meta.created));
    }
    info.push_str(" >>");
    objects.push((3, info.into_bytes()));

    for (index, page) in pages.iter().enumerate() {
        let page_id = page_ids[index];
        let content_id = page_id + 1;
        let image_id = page_id + 2;

        let (info, data, filter) = match page.kind {
            ImageKind::Jpeg => {
                let info = jpeg_info(&page.bytes)?;
                (info, page.bytes.clone(), "/DCTDecode".to_string())
            }
            ImageKind::Png => {
                let (info, idat) = png_parts(&page.bytes)?;
                let parms = format!(
                    "/FlateDecode /DecodeParms << /Predictor 15 /Colors {} /BitsPerComponent {} /Columns {} >>",
                    info.components, info.bits, info.width
                );
                (info, idat, parms)
            }
        };

        let width = points(info.width, page.dpi);
        let height = points(info.height, page.dpi);
        objects.push((
            page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:.2} {height:.2}] \
                 /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        ));

        let draw = format!("q {width:.2} 0 0 {height:.2} 0 0 cm /Im0 Do Q\n");
        let mut content = format!("<< /Length {} >>\nstream\n", draw.len()).into_bytes();
        content.extend_from_slice(draw.as_bytes());
        content.extend_from_slice(b"endstream");
        objects.push((content_id, content));

        let colour_space = match info.components {
            1 => "/DeviceGray",
            3 => "/DeviceRGB",
            _ => "/DeviceCMYK",
        };
        // JPEG is always 8 bits per component as far as PDF is concerned.
        let bits = if page.kind == ImageKind::Jpeg { 8 } else { info.bits };
        let decode = if info.adobe_inverted && info.components == 4 { " /Decode [1 0 1 0 1 0 1 0]" } else { "" };
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {colour_space} \
             /BitsPerComponent {bits}{decode} /Filter {filter} /Length {} >>\nstream\n",
            info.width,
            info.height,
            data.len()
        )
        .into_bytes();
        image.extend_from_slice(&data);
        image.extend_from_slice(b"\nendstream");
        objects.push((image_id, image));
    }

    objects.sort_by_key(|(id, _)| *id);
    for (id, body) in &objects {
        offsets.push(out.len());
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }

    let xref = out.len();
    let count = objects.len() + 1;
    let mut table = format!("xref\n0 {count}\n0000000000 65535 f \n");
    for offset in &offsets {
        let _ = write!(table, "{offset:010} 00000 n \n");
    }
    let _ = write!(table, "trailer\n<< /Size {count} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out.extend_from_slice(table.as_bytes());
    Ok(out)
}

/// The PDF date for a moment, in UTC.
pub fn pdf_date(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("D:%Y%m%d%H%M%SZ").to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny but real baseline JPEG: 16x8, three components.
    pub fn sample_jpeg(width: u16, height: u16, components: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        // APP0 JFIF
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
        // SOF0
        let len = 8 + 3 * components as u16;
        v.extend_from_slice(&[0xFF, 0xC0]);
        v.extend_from_slice(&len.to_be_bytes());
        v.push(8);
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&width.to_be_bytes());
        v.push(components);
        for c in 0..components {
            v.extend_from_slice(&[c + 1, 0x11, 0]);
        }
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    fn chunk(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = (body.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v.extend_from_slice(&[0, 0, 0, 0]); // CRC is not checked here
        v
    }

    pub fn sample_png(width: u32, height: u32, bits: u8, colour: u8, idat: &[&[u8]]) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = width.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[bits, colour, 0, 0, 0]);
        v.extend(chunk(b"IHDR", &ihdr));
        for part in idat {
            v.extend(chunk(b"IDAT", part));
        }
        v.extend(chunk(b"IEND", &[]));
        v
    }

    #[test]
    fn reads_the_frame_header_of_a_jpeg() {
        let info = jpeg_info(&sample_jpeg(2480, 3508, 3)).unwrap();
        assert_eq!((info.width, info.height, info.components, info.bits), (2480, 3508, 3, 8));
        assert!(!info.adobe_inverted);
        assert_eq!(jpeg_info(&sample_jpeg(10, 10, 1)).unwrap().components, 1);
    }

    #[test]
    fn notices_an_adobe_cmyk_jpeg() {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xEE, 0x00, 0x0E];
        data.extend_from_slice(b"Adobe\0\x64\0\0\0\0\x02");
        data.extend_from_slice(&sample_jpeg(4, 4, 4)[2..]);
        let info = jpeg_info(&data).unwrap();
        assert_eq!(info.components, 4);
        assert!(info.adobe_inverted);
    }

    #[test]
    fn refuses_what_is_not_a_jpeg() {
        assert!(jpeg_info(b"").is_err());
        assert!(jpeg_info(b"\x89PNG\r\n\x1a\n").is_err());
        // Truncated inside a segment.
        assert!(jpeg_info(&sample_jpeg(4, 4, 3)[..8]).is_err());
    }

    #[test]
    fn png_image_data_is_joined_across_chunks() {
        let png = sample_png(3, 2, 8, 0, &[b"abc", b"def"]);
        let (info, idat) = png_parts(&png).unwrap();
        assert_eq!((info.width, info.height, info.components, info.bits), (3, 2, 1, 8));
        assert_eq!(idat, b"abcdef");
    }

    #[test]
    fn png_shapes_a_predictor_cannot_read_are_refused() {
        // Palette, grey+alpha, RGBA.
        for colour in [3u8, 4, 6] {
            assert!(png_parts(&sample_png(1, 1, 8, colour, &[b"x"])).is_err(), "colour type {colour}");
        }
        let mut interlaced = sample_png(1, 1, 8, 0, &[b"x"]);
        interlaced[8 + 8 + 12] = 1;
        assert!(png_parts(&interlaced).is_err());
        assert!(png_parts(&sample_png(1, 1, 8, 0, &[])).is_err(), "no image data");
    }

    #[test]
    fn page_size_comes_from_pixels_and_resolution() {
        // A4 at 300 dpi is 2480 x 3508 pixels: 595 x 842 points.
        assert!((points(2480, 300) - 595.2).abs() < 0.01);
        assert!((points(3508, 300) - 841.92).abs() < 0.01);
        // No resolution reported: assume 300 rather than divide by zero.
        assert_eq!(points(300, 0), 72.0);
    }

    #[test]
    fn text_strings_escape_or_go_to_utf16() {
        assert_eq!(pdf_text("Tax (2026)"), "(Tax \\(2026\\))");
        assert_eq!(pdf_text("a\\b"), "(a\\\\b)");
        assert_eq!(pdf_text("Café"), "<FEFF00430061006600E9>");
    }

    #[test]
    fn writes_a_pdf_whose_cross_reference_table_points_at_its_objects() {
        let pages = vec![
            Page { kind: ImageKind::Jpeg, bytes: sample_jpeg(2480, 3508, 3), dpi: 300 },
            Page { kind: ImageKind::Png, bytes: sample_png(8, 8, 1, 0, &[b"zz"]), dpi: 75 },
        ];
        let meta = Meta { title: "Scan".into(), keywords: vec!["Tax".into(), "Home".into()], created: "D:20261108120000Z".into() };
        let pdf = build(&pages, &meta).unwrap();
        let text = String::from_utf8_lossy(&pdf);

        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/Keywords (Tax, Home)"));
        assert!(text.contains("/Filter /DCTDecode"));
        assert!(text.contains("/Predictor 15 /Colors 1 /BitsPerComponent 1 /Columns 8"));
        assert!(text.contains("/MediaBox [0 0 595.20 841.92]"));

        // Every xref entry must land exactly on "N 0 obj". Offsets are byte
        // offsets, so this works on the bytes: the header and the images are
        // binary, and decoding them as text would shift every position.
        let marker = b"startxref\n";
        let at = pdf.windows(marker.len()).rposition(|w| w == marker).unwrap();
        let tail = std::str::from_utf8(&pdf[at + marker.len()..]).unwrap();
        let start: usize = tail.lines().next().unwrap().parse().unwrap();
        let table = std::str::from_utf8(&pdf[start..]).unwrap();
        assert!(table.starts_with("xref\n"));
        let entries: Vec<&str> = table.lines().skip(3).take_while(|l| l.ends_with(" n ")).collect();
        assert_eq!(entries.len(), 3 + 2 * 3);
        for (n, entry) in entries.iter().enumerate() {
            let offset: usize = entry[..10].parse().unwrap();
            assert!(pdf[offset..].starts_with(format!("{} 0 obj", n + 1).as_bytes()), "object {}", n + 1);
        }
    }

    #[test]
    fn an_empty_scan_is_not_a_document() {
        assert!(build(&[], &Meta::default()).is_err());
    }
}
